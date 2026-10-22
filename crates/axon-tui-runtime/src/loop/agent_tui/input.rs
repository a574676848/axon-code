//! Agent TUI 结构化输入契约。

use std::path::{Path, PathBuf};

use super::attachments::{AgentTuiAttachmentKind, compact_path};

/// Agent TUI 提交、命令与队列动作的统一输入结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentTuiInputResult {
    /// 普通用户任务提交。
    Submit {
        /// 提交给运行时的最终文本。
        text: String,
        /// 输入区中的结构化元素。
        elements: Vec<AgentTuiInputElement>,
    },
    /// 不进入模型提示词的 TUI 控制命令，由宿主 CLI 执行真实运行时动作。
    ControlCommand {
        /// slash command 名称，例如 `/status`。
        command: String,
        /// command 后的参数文本。
        args: String,
        /// UI 中展示和历史记录的文本。
        text: String,
    },
    /// 运行中排队的下一轮输入。
    Queued {
        /// 提交给运行时的最终文本。
        text: String,
        /// 输入区中的结构化元素。
        elements: Vec<AgentTuiInputElement>,
        /// 队列动作语义。
        action: AgentTuiQueuedAction,
    },
}

/// 运行中队列的动作类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentTuiQueuedAction {
    /// 普通下一轮任务。
    Plain,
    /// 下一轮应按 shell 运行语义处理。
    RunShell,
}

/// 输入区元素，承载文件、图片、mention 和大段粘贴占位。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTuiInputElement {
    /// 元素在输入文本中的占位文本。
    pub placeholder: String,
    /// 元素类型与载荷。
    pub kind: AgentTuiInputElementKind,
}

/// 输入元素类型。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentTuiInputElementKind {
    /// 本地文件附件。
    File {
        /// 文件路径。
        path: PathBuf,
    },
    /// 本地图片附件。
    Image {
        /// 图片路径。
        path: PathBuf,
    },
    /// 显式 mention，用于后续绑定 Skill、connector 或文件目标。
    Mention {
        /// mention 前缀符号，例如 `@` 或 `$`。
        sigil: char,
        /// mention 展示名。
        name: String,
        /// mention 解析后的目标标识。
        target: String,
    },
    /// 大段粘贴占位。
    LargePaste {
        /// 占位对应的原始粘贴内容。
        content: String,
    },
}

impl AgentTuiInputResult {
    /// 提交给真实运行时的文本。
    #[must_use]
    pub fn text(&self) -> &str {
        match self {
            Self::Submit { text, .. }
            | Self::Queued { text, .. }
            | Self::ControlCommand { text, .. } => text,
        }
    }

    /// 输入区结构化元素。
    #[must_use]
    pub fn elements(&self) -> &[AgentTuiInputElement] {
        match self {
            Self::Submit { elements, .. } | Self::Queued { elements, .. } => elements,
            Self::ControlCommand { .. } => &[],
        }
    }

    /// 投影为运行时当前支持的附件路径。
    #[must_use]
    pub fn file_paths(&self) -> Vec<PathBuf> {
        self.elements()
            .iter()
            .filter_map(AgentTuiInputElement::path)
            .collect()
    }
}

impl AgentTuiInputElement {
    /// 构造文件或图片附件元素。
    #[must_use]
    pub(crate) fn attachment(index: usize, path: PathBuf, kind: AgentTuiAttachmentKind) -> Self {
        let placeholder = match kind {
            AgentTuiAttachmentKind::File => file_placeholder(index, &path),
            AgentTuiAttachmentKind::Image => image_placeholder(index, &path),
        };
        let kind = match kind {
            AgentTuiAttachmentKind::File => AgentTuiInputElementKind::File { path },
            AgentTuiAttachmentKind::Image => AgentTuiInputElementKind::Image { path },
        };
        Self { placeholder, kind }
    }

    /// 构造大段粘贴占位元素。
    #[must_use]
    pub(crate) fn large_paste(placeholder: String, content: String) -> Self {
        Self {
            placeholder,
            kind: AgentTuiInputElementKind::LargePaste { content },
        }
    }

    /// 构造显式 mention 元素。
    #[must_use]
    pub(crate) fn mention(sigil: char, name: String, target: String) -> Self {
        Self {
            placeholder: format!("{sigil}{name}"),
            kind: AgentTuiInputElementKind::Mention {
                sigil,
                name,
                target,
            },
        }
    }

    fn path(&self) -> Option<PathBuf> {
        match &self.kind {
            AgentTuiInputElementKind::File { path } | AgentTuiInputElementKind::Image { path } => {
                Some(path.clone())
            }
            AgentTuiInputElementKind::Mention { .. }
            | AgentTuiInputElementKind::LargePaste { .. } => None,
        }
    }
}

pub(crate) fn file_placeholder(index: usize, path: &Path) -> String {
    format!(
        "[File #{}] {}",
        index + 1,
        compact_path(&path.display().to_string())
    )
}

pub(crate) fn image_placeholder(index: usize, _path: &Path) -> String {
    format!("[Image #{}]", index + 1)
}
