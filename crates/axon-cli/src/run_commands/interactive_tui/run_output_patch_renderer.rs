//! Durable wire 文件变更事实到 diff 行的投影。

use std::collections::BTreeMap;
use std::path::PathBuf;

use axon_protocol::diff::FileChange;
use axon_protocol::wire::WireFileChangeEvent;
use axon_tui_runtime::AgentTuiDiffLine;

const DIFF_TITLE: &str = "文件变更";

/// 同一次工具写入内的同一路径视为同一份事实，用于按 sequence 收敛。
pub(super) fn wire_file_change_key(change: &WireFileChangeEvent) -> (String, String) {
    let source = change
        .tool_part_id
        .as_deref()
        .filter(|part_id| !part_id.trim().is_empty())
        .unwrap_or(change.part_id.as_str())
        .to_owned();
    (source, change.path.trim().replace('\\', "/"))
}

pub(super) fn wire_file_change_diff(event: &WireFileChangeEvent) -> Option<AgentTuiDiffLine> {
    let part_id = event.part_id.trim();
    if part_id.is_empty() {
        return None;
    }
    let mut changes = BTreeMap::new();
    changes.insert(PathBuf::from(&event.path), event.change.clone());
    let renames = usize::from(matches!(
        &event.change,
        FileChange::Update {
            move_path: Some(_),
            ..
        }
    ));
    Some(AgentTuiDiffLine::with_id(
        part_id, DIFF_TITLE, changes, 0, renames,
    ))
}
