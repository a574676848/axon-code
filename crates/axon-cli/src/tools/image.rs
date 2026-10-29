//! `builtin.read_file` 的图片分支。
//!
//! 图片不单独暴露工具：`read_file` 命中受支持图片时先走这里，完成路径解析、格式判定、
//! 字节上限与附件归档，再把图片本体交给工具回执的 `localImages`；未命中图片时返回
//! `None`，由文本分支按原契约继续读取。

use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

use axon_agent_runtime::{
    AgentModelAttachmentProjection, AttachmentBlobStore, AttachmentIngressError,
    AttachmentIngressRequest, AttachmentIngressRuntime, ProviderToolCall,
    READ_FILE_IMAGE_FORMAT_CONTRACT, READ_FILE_IMAGE_SNIFF_BYTES, READ_FILE_IMAGE_SOURCE_MAX_BYTES,
    READ_FILE_IMAGE_SOURCE_PREFIX, READ_FILE_TOOL_PATH_FIELD, ReadFileImageReceipt,
    agent_run_read_file_image_receipt, default_attachment_archive_root, image_mime_type_for_path,
    normalize_image, resolve_existing_tool_path, sniff_image_mime_type,
};
use axon_domain::{AttachmentKind, AttachmentSensitivity, AttachmentSource};

use crate::tool_orchestrator::{
    ToolOrchestratorContext, ToolReceipt, failed_tool_input_receipt, failed_tool_receipt, now_ms,
    tool_receipt_from_runtime,
};

const READ_FILE_IMAGE_ARCHIVE_UNAVAILABLE: &str = "图片附件归档目录不可用，本次未读取图片";
const READ_FILE_IMAGE_MODEL_WITHOUT_VISION: &str = "当前模型未声明图片输入能力，图片无法交给模型判断；请改用文本读取路径，或切换到支持视觉输入的模型后重试";

/// 读取路径命中受支持图片时执行图片分支。
///
/// 返回 `None` 表示该路径不是图片（或参数由文本分支负责报错），调用方应继续走文本读取。
pub(crate) fn execute_read_file_image(
    tool_call: &ProviderToolCall,
    context: &ToolOrchestratorContext,
) -> Option<ToolReceipt> {
    let image = detect_image_target(tool_call, context)?;
    let Some(archive_root) = default_attachment_archive_root() else {
        return Some(failed_tool_receipt(
            tool_call,
            READ_FILE_IMAGE_ARCHIVE_UNAVAILABLE,
        ));
    };
    Some(execute_detected_image(
        tool_call,
        context,
        &image,
        &archive_root,
    ))
}

#[cfg(test)]
pub(crate) fn execute_read_file_image_with_archive_root(
    tool_call: &ProviderToolCall,
    context: &ToolOrchestratorContext,
    archive_root: &Path,
) -> Option<ToolReceipt> {
    let image = detect_image_target(tool_call, context)?;
    Some(execute_detected_image(
        tool_call,
        context,
        &image,
        archive_root,
    ))
}

/// 已判定为图片的读取目标。
struct ImageTarget {
    target: PathBuf,
    raw_path: String,
    /// 扩展名声明的图片 MIME；无扩展名或非图片扩展名时为空。
    declared: Option<&'static str>,
}

fn detect_image_target(
    tool_call: &ProviderToolCall,
    context: &ToolOrchestratorContext,
) -> Option<ImageTarget> {
    let value = serde_json::from_str::<serde_json::Value>(&tool_call.canonical_input()).ok()?;
    let raw_path = value
        .get(READ_FILE_TOOL_PATH_FIELD)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|path| !path.is_empty())?;
    let path = PathBuf::from(raw_path.replace('\\', "/"));
    let declared = image_mime_type_for_path(&path);
    let target = resolve_existing_tool_path(&context.cwd, &path).ok()?;
    if !target.is_file() {
        return None;
    }
    // 只有拿到图片正据才接手指令：扩展名声明是图片，或没有可识别扩展名时签名是图片。
    // 其余文件继续走文本分支，普通无扩展名文本不受影响。
    if declared.is_none() && sniff_image_mime_type(&read_leading_bytes(&target)?).is_none() {
        return None;
    }
    Some(ImageTarget {
        target,
        raw_path: raw_path.to_owned(),
        declared,
    })
}

fn execute_detected_image(
    tool_call: &ProviderToolCall,
    context: &ToolOrchestratorContext,
    image: &ImageTarget,
    archive_root: &Path,
) -> ToolReceipt {
    read_image(tool_call, context, image, archive_root)
}

fn read_image(
    tool_call: &ProviderToolCall,
    context: &ToolOrchestratorContext,
    image: &ImageTarget,
    archive_root: &Path,
) -> ToolReceipt {
    let ImageTarget {
        target,
        raw_path,
        declared,
    } = image;
    // 签名是图片但扩展名不受支持：格式事实与文件名冲突，要求按真实格式重命名。
    if let Some(extension) = target.extension().and_then(|value| value.to_str())
        && declared.is_none()
    {
        return failed_tool_input_receipt(
            tool_call,
            format!(
                "无法读取 \"{raw_path}\"：.{extension} 扩展名不是受支持的图片格式；read_file 只接受 {READ_FILE_IMAGE_FORMAT_CONTRACT}，无扩展名但内容合法的图片同样可读"
            ),
        );
    }
    let display_path = display_path(&context.cwd, target);
    if !context.supports_vision {
        return failed_tool_receipt(tool_call, READ_FILE_IMAGE_MODEL_WITHOUT_VISION);
    }
    let byte_len = match fs::metadata(target) {
        Ok(metadata) => metadata.len(),
        Err(_) => {
            return failed_tool_receipt(
                tool_call,
                format!("无法读取 \"{display_path}\"：文件不可访问"),
            );
        }
    };
    if byte_len > READ_FILE_IMAGE_SOURCE_MAX_BYTES {
        return failed_tool_receipt(tool_call, oversize_message(&display_path, byte_len));
    }
    let bytes = match fs::read(target) {
        Ok(bytes) => bytes,
        Err(_) => {
            return failed_tool_receipt(
                tool_call,
                format!("无法读取 \"{display_path}\"：文件不可访问"),
            );
        }
    };
    if bytes.is_empty() {
        return failed_tool_input_receipt(
            tool_call,
            format!("无法读取 \"{display_path}\"：文件内容为空，不是图片"),
        );
    }
    let Some(mime_type) = sniff_image_mime_type(&bytes) else {
        return failed_tool_input_receipt(
            tool_call,
            format!(
                "无法读取 \"{display_path}\"：文件内容不是受支持的图片格式；read_file 只接受 {READ_FILE_IMAGE_FORMAT_CONTRACT}，文件可能已损坏或被截断"
            ),
        );
    };
    if let Some(declared) = declared
        && *declared != mime_type
    {
        return failed_tool_input_receipt(
            tool_call,
            format!(
                "无法读取 \"{display_path}\"：扩展名声明 {declared}，但文件内容实际是 {mime_type}；请按真实格式重命名或转换后再读取"
            ),
        );
    }
    // 统一归一化：应用 EXIF 方向、按像素预算与长边上限降采样、编码到字节预算内。
    let normalized = match normalize_image(&bytes, mime_type) {
        Ok(normalized) => normalized,
        Err(error) => {
            return failed_tool_receipt(tool_call, format!("无法读取 \"{display_path}\"：{error}"));
        }
    };
    let source_ref = format!("{READ_FILE_IMAGE_SOURCE_PREFIX}{display_path}");
    let normalized_byte_len = normalized.bytes.len();
    let archived = match archive_image(
        archive_root,
        normalized.bytes,
        normalized.mime_type,
        &source_ref,
    ) {
        Ok(archived) => archived,
        Err(message) => return failed_tool_receipt(tool_call, message),
    };
    let receipt = ReadFileImageReceipt {
        path: display_path,
        absolute_path: absolute_display_path(target),
        mime_type: normalized.mime_type.to_owned(),
        byte_len: normalized_byte_len,
        width: normalized.width,
        height: normalized.height,
        original_width: normalized.original_width,
        original_height: normalized.original_height,
        sha256: archived.sha256,
        archive_ref: archived.archive_ref,
        source_ref: archived.source_ref,
    };
    tool_receipt_from_runtime(agent_run_read_file_image_receipt(tool_call, &receipt))
}

/// 归档成功后的图片引用。
struct ArchivedImage {
    sha256: String,
    archive_ref: String,
    source_ref: String,
}

fn archive_image(
    archive_root: &Path,
    bytes: Vec<u8>,
    mime_type: &str,
    source_ref: &str,
) -> Result<ArchivedImage, String> {
    let mut store = AttachmentBlobStore::new(archive_root);
    let mut ingress = AttachmentIngressRuntime::default();
    let record = ingress
        .ingest_with_blob_store(
            AttachmentIngressRequest {
                source: AttachmentSource::ToolOutputFile,
                kind: AttachmentKind::Image,
                mime_type: mime_type.to_owned(),
                bytes,
                source_trace: source_ref.to_owned(),
                sensitivity: AttachmentSensitivity::Normal,
            },
            &mut store,
            now_ms(),
        )
        .map_err(ingress_error)?;
    let image = match AttachmentIngressRuntime::project_for_model(&record.attachment) {
        Ok(AgentModelAttachmentProjection::Image(image)) => image,
        Ok(_) => return Err("图片归档结果不是图片引用，本次未交给模型".to_owned()),
        Err(error) => return Err(format!("图片归档失败：{error}")),
    };
    let Some(archive_ref) = image.archive_ref else {
        return Err("图片归档引用缺失，provider 无法读回该图片".to_owned());
    };
    Ok(ArchivedImage {
        sha256: record.attachment.sha256,
        archive_ref,
        source_ref: image.source_ref.unwrap_or_else(|| source_ref.to_owned()),
    })
}

fn ingress_error(error: AttachmentIngressError) -> String {
    match error {
        AttachmentIngressError::PayloadTooLarge { limit, actual } => {
            format!("图片 {actual} 字节超过 {limit} 字节上限；请先缩小图片再读取")
        }
        error => format!("图片归档失败：{error}"),
    }
}

fn oversize_message(display_path: &str, actual: u64) -> String {
    format!(
        "无法读取 \"{display_path}\"：图片 {actual} 字节超过 {READ_FILE_IMAGE_SOURCE_MAX_BYTES} 字节上限；请先缩小图片再读取"
    )
}

/// 只读文件头部用于签名判定，避免为一次判定读入整张图片。
fn read_leading_bytes(target: &Path) -> Option<Vec<u8>> {
    let mut file = fs::File::open(target).ok()?;
    let mut buffer = vec![0_u8; READ_FILE_IMAGE_SNIFF_BYTES];
    let mut filled = 0;
    while filled < buffer.len() {
        match file.read(&mut buffer[filled..]) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return None,
        }
    }
    buffer.truncate(filled);
    Some(buffer)
}

/// 供回执与模型可见路径使用的绝对路径：去掉 Windows canonicalize 的 verbatim 前缀，统一为正斜杠。
fn absolute_display_path(target: &Path) -> String {
    let raw = target.to_string_lossy();
    let trimmed = raw
        .strip_prefix(r"\\?\")
        .or_else(|| raw.strip_prefix("//?/"))
        .unwrap_or(&raw);
    trimmed.replace('\\', "/")
}

fn display_path(cwd: &Path, target: &Path) -> String {
    let root = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    target
        .strip_prefix(&root)
        .unwrap_or(target)
        .to_string_lossy()
        .replace('\\', "/")
}

#[cfg(test)]
#[path = "image_tests.rs"]
mod tests;
