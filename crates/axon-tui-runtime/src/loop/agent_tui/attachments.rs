use std::path::{Path, PathBuf};

const ATTACHMENT_FILE_NOT_FOUND: &str = "附件路径不存在或不可访问";
const ATTACHMENT_IMAGE_KIND_ERROR: &str = "图片附件必须是 png、jpg、jpeg、gif、webp 或 bmp 文件";
const ATTACHMENT_FILE_KIND_ERROR: &str = "文件附件不能是目录";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentTuiAttachment {
    pub(crate) path: String,
    pub(crate) kind: AgentTuiAttachmentKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentTuiAttachmentKind {
    File,
    Image,
}

impl AgentTuiAttachment {
    pub(crate) fn new(path: impl Into<String>, kind: AgentTuiAttachmentKind) -> Self {
        Self {
            path: path.into(),
            kind,
        }
    }

    pub(crate) fn as_path_buf(&self) -> PathBuf {
        PathBuf::from(&self.path)
    }
}

pub(super) fn validated_attachment(
    value: &str,
    expected_kind: AgentTuiAttachmentKind,
) -> Result<AgentTuiAttachment, &'static str> {
    let trimmed = value.trim().trim_matches('"').trim_matches('\'');
    let path = PathBuf::from(trimmed);
    let canonical = path.canonicalize().map_err(|_| ATTACHMENT_FILE_NOT_FOUND)?;
    if canonical.is_dir() {
        return Err(ATTACHMENT_FILE_KIND_ERROR);
    }
    if expected_kind == AgentTuiAttachmentKind::Image && !is_image_path(&canonical) {
        return Err(ATTACHMENT_IMAGE_KIND_ERROR);
    }
    Ok(AgentTuiAttachment::new(
        canonical.display().to_string(),
        expected_kind,
    ))
}

pub(super) fn infer_attachment_kind(path: &Path) -> AgentTuiAttachmentKind {
    if is_image_path(path) {
        AgentTuiAttachmentKind::Image
    } else {
        AgentTuiAttachmentKind::File
    }
}

fn is_image_path(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp"
    )
}

pub(crate) fn compact_path(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(path)
        .to_owned()
}
