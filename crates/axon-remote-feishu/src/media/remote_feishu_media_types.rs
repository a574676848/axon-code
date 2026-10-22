//! Feishu 媒体引用类型。

use serde::{Deserialize, Serialize};

/// Feishu 媒体资源类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RemoteFeishuMediaKind {
    /// 图片。
    Image,
    /// 文件。
    File,
    /// 音频。
    Audio,
    /// 视频或富媒体。
    Media,
    /// 表情贴纸。
    Sticker,
}

impl RemoteFeishuMediaKind {
    /// 稳定文本标签。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::File => "file",
            Self::Audio => "audio",
            Self::Media => "media",
            Self::Sticker => "sticker",
        }
    }
}

/// Feishu 入站媒体引用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuMediaRef {
    /// 媒体类型。
    pub kind: RemoteFeishuMediaKind,
    /// Feishu file_key / image_key / media_key。
    pub key: String,
    /// 文件名。
    pub file_name: Option<String>,
}
