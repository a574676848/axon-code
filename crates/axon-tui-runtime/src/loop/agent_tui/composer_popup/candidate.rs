//! Composer popup 候选项模型。

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ComposerPopupKind {
    File,
    Image,
    Path,
    Shell,
    Skill,
    Connector,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ComposerPopupMentionType {
    Skill,
    File,
    Image,
    Directory,
    Shell,
    Connector,
}

impl ComposerPopupMentionType {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Skill => "skill",
            Self::File => "file",
            Self::Image => "image",
            Self::Directory => "directory",
            Self::Shell => "shell",
            Self::Connector => "connector",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ComposerPopupItem {
    pub(crate) value: String,
    pub(crate) description: String,
    pub(crate) insert_text: String,
    pub(crate) target: String,
    pub(crate) mention_type: ComposerPopupMentionType,
    pub(crate) match_indices: Option<Vec<usize>>,
    pub(crate) score: i32,
}
