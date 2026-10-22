//! Transcript Diff cell 的结构化数据模型。

use std::{collections::BTreeMap, path::PathBuf};

use axon_protocol::diff::FileChange;

use crate::AgentTuiDiffLine;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentRenderDiffCell {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) changes: BTreeMap<PathBuf, FileChange>,
    pub(crate) diagnostics: BTreeMap<PathBuf, Vec<String>>,
    pub(crate) text_writes_applied: usize,
    pub(crate) renames_applied: usize,
    pub(crate) truncated: bool,
    pub(crate) truncation_boundary: Option<String>,
}

impl AgentRenderDiffCell {
    pub(crate) fn from_tui_line(line: &AgentTuiDiffLine) -> Self {
        Self {
            id: line.id.clone(),
            title: line.title.clone(),
            changes: line.changes.clone(),
            diagnostics: line.diagnostics.clone(),
            text_writes_applied: line.text_writes_applied,
            renames_applied: line.renames_applied,
            truncated: line.truncated,
            truncation_boundary: line.truncation_boundary.clone(),
        }
    }

    pub(crate) fn has_diff_content(&self) -> bool {
        self.changes.values().any(file_change_has_diff_content)
    }
}

fn file_change_has_diff_content(change: &FileChange) -> bool {
    match change {
        FileChange::Add { content } | FileChange::Delete { content } => !content.is_empty(),
        FileChange::Update { unified_diff, .. } => unified_diff
            .lines()
            .any(|line| !line.is_empty() && !line.starts_with("---") && !line.starts_with("+++")),
    }
}

pub(crate) fn file_change_label(change: &FileChange) -> &'static str {
    match change {
        FileChange::Add { .. } => "Added",
        FileChange::Delete { .. } => "Deleted",
        FileChange::Update {
            move_path: Some(_), ..
        } => "Renamed",
        FileChange::Update { .. } => "Writed",
    }
}

pub(crate) fn rendered_path(path: &std::path::Path, change: &FileChange) -> String {
    match change {
        FileChange::Update {
            move_path: Some(target),
            ..
        } if target != path => format!("{} -> {}", path.display(), target.display()),
        _ => path.display().to_string(),
    }
}

pub(crate) fn line_stats(change: &FileChange) -> (usize, usize) {
    match change {
        FileChange::Add { content } => (content_line_count(content), 0),
        FileChange::Delete { content } => (0, content_line_count(content)),
        FileChange::Update { unified_diff, .. } => unified_diff_line_stats(unified_diff),
    }
}

pub(crate) fn raw_diff_lines(change: &FileChange) -> Vec<String> {
    match change {
        FileChange::Add { content } => content.lines().map(|line| format!("+{line}")).collect(),
        FileChange::Delete { content } => content.lines().map(|line| format!("-{line}")).collect(),
        FileChange::Update { unified_diff, .. } => unified_diff
            .lines()
            .filter(|line| !line.starts_with("---") && !line.starts_with("+++"))
            .map(ToOwned::to_owned)
            .collect(),
    }
}

fn unified_diff_line_stats(diff: &str) -> (usize, usize) {
    let mut added = 0usize;
    let mut removed = 0usize;
    for line in diff.lines() {
        if line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        if line.starts_with('+') {
            added = added.saturating_add(1);
        } else if line.starts_with('-') {
            removed = removed.saturating_add(1);
        }
    }
    (added, removed)
}

fn content_line_count(value: &str) -> usize {
    value.lines().count().max(usize::from(!value.is_empty()))
}
