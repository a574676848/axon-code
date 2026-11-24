//! TUI Diff 运行结果模型。

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use axon_protocol::diff::FileChange;
use serde_json::Value;

/// TUI 一等 Diff 运行结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTuiDiffLine {
    /// 稳定 diff part id。
    pub id: String,
    /// Diff cell 标题。
    pub title: String,
    /// 文件变更列表，key 为源路径；重命名目标放在 `FileChange::Update::move_path`。
    pub changes: BTreeMap<PathBuf, FileChange>,
    /// 文件级诊断摘要。
    pub diagnostics: BTreeMap<PathBuf, Vec<String>>,
    /// 文本写入次数。
    pub text_writes_applied: usize,
    /// 重命名次数。
    pub renames_applied: usize,
    /// 是否发生 diff 截断。
    pub truncated: bool,
    /// 截断边界说明。
    pub truncation_boundary: Option<String>,
}

impl AgentTuiDiffLine {
    /// 创建一等 Diff 运行结果。
    #[must_use]
    pub fn with_id(
        id: impl Into<String>,
        title: impl Into<String>,
        changes: BTreeMap<PathBuf, FileChange>,
        text_writes_applied: usize,
        renames_applied: usize,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            changes,
            diagnostics: BTreeMap::new(),
            text_writes_applied,
            renames_applied,
            truncated: false,
            truncation_boundary: None,
        }
    }

    /// 测试夹具构造器。生产路径必须传入自然 part id。
    #[cfg(test)]
    #[must_use]
    pub fn new(
        title: impl Into<String>,
        changes: BTreeMap<PathBuf, FileChange>,
        text_writes_applied: usize,
        renames_applied: usize,
    ) -> Self {
        Self::with_id(
            "diff:test",
            title,
            changes,
            text_writes_applied,
            renames_applied,
        )
    }

    /// 附加文件级诊断。
    #[must_use]
    pub fn with_diagnostics(mut self, diagnostics: BTreeMap<PathBuf, Vec<String>>) -> Self {
        self.diagnostics = diagnostics;
        self
    }

    /// 附加截断信息。
    #[must_use]
    pub fn with_truncation(mut self, truncated: bool, truncation_boundary: Option<String>) -> Self {
        self.truncated = truncated;
        self.truncation_boundary = truncation_boundary;
        self
    }
}

/// 从 Codex/FileChange 风格字段构造文件级变更。
#[must_use]
pub fn file_change_from_parts(
    path: impl AsRef<Path>,
    kind: &Value,
    diff: &str,
) -> Option<FileChange> {
    let kind_name = change_kind(kind);
    match kind_name.as_str() {
        "add" => Some(FileChange::Add {
            content: diff.to_owned(),
        }),
        "delete" => Some(FileChange::Delete {
            content: diff.to_owned(),
        }),
        "move" => Some(FileChange::Update {
            unified_diff: diff.to_owned(),
            move_path: move_path(kind).map(PathBuf::from),
        }),
        _ => Some(FileChange::Update {
            unified_diff: diff.to_owned(),
            move_path: move_path(kind)
                .filter(|target| Path::new(target) != path.as_ref())
                .map(PathBuf::from),
        }),
    }
}

fn change_kind(value: &Value) -> String {
    if let Some(kind) = value.as_str() {
        return normalized_kind(kind);
    }
    value
        .get("type")
        .or_else(|| value.get("operation"))
        .and_then(Value::as_str)
        .map(normalized_kind)
        .unwrap_or_else(|| "update".to_owned())
}

fn normalized_kind(value: &str) -> String {
    match value
        .replace(['_', '-', '/'], "")
        .to_ascii_lowercase()
        .as_str()
    {
        "added" | "add" | "created" | "create" | "new" => "add".to_owned(),
        "deleted" | "delete" | "removed" | "remove" => "delete".to_owned(),
        "renamed" | "rename" | "moved" | "move" => "move".to_owned(),
        _ => "update".to_owned(),
    }
}

fn move_path(kind: &Value) -> Option<String> {
    kind.get("movePath")
        .or_else(|| kind.get("move_path"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(ToOwned::to_owned)
}
