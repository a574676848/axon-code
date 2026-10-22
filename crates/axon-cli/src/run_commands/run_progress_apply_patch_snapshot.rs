//! apply_patch begin 阶段的文件 diff 快照投影。

use std::{collections::BTreeMap, fmt::Write as _, path::PathBuf};

use axon_agent_runtime::{
    agent_run_edit_input_is_apply_patch, agent_run_edit_tool_args_from_provider_input,
};
use axon_patch_model::{PatchHunk, UpdateFileChunk, parse_patch};
use axon_protocol::diff::FileChange;

use crate::tool_orchestrator::{ToolExecutionRequest, ToolKind};

use super::run_progress::RunToolDiffSnapshotView;

const APPLY_PATCH_DIFF_OLD_PATH_PREFIX: &str = "--- ";
const APPLY_PATCH_DIFF_NEW_PATH_PREFIX: &str = "+++ ";
const APPLY_PATCH_DIFF_HUNK_PREFIX: &str = "@@";

pub(super) fn apply_patch_diff_snapshot_from_execution(
    execution_request: &ToolExecutionRequest,
) -> Option<RunToolDiffSnapshotView> {
    if execution_request.kind != ToolKind::Edit {
        return None;
    }
    let input =
        agent_run_edit_tool_args_from_provider_input(&execution_request.tool_call.input).ok()?;
    if !agent_run_edit_input_is_apply_patch(&input.input) {
        return None;
    }
    apply_patch_diff_snapshot(
        &execution_request.tool_call.id,
        &execution_request.part_id,
        &input.input,
    )
}

pub(super) fn apply_patch_diff_snapshot(
    provider_call_id: &str,
    tool_part_id: &str,
    patch: &str,
) -> Option<RunToolDiffSnapshotView> {
    let changes = file_changes_from_apply_patch(patch)?;
    Some(RunToolDiffSnapshotView {
        tool_part_id: tool_part_id.to_owned(),
        provider_call_id: provider_call_id.to_owned(),
        changes,
    })
}

fn file_changes_from_apply_patch(patch: &str) -> Option<BTreeMap<PathBuf, FileChange>> {
    let mut changes = BTreeMap::new();
    for hunk in parse_patch(patch).ok()? {
        match hunk {
            PatchHunk::AddFile { path, contents } => {
                changes.insert(PathBuf::from(path), FileChange::Add { content: contents });
            }
            PatchHunk::DeleteFile { path } => {
                changes.insert(
                    PathBuf::from(path),
                    FileChange::Delete {
                        content: String::new(),
                    },
                );
            }
            PatchHunk::UpdateFile {
                path,
                move_path,
                chunks,
            } => {
                let target_path = move_path.as_deref().unwrap_or(&path);
                let unified_diff =
                    unified_diff_from_apply_patch_chunks(&path, target_path, &chunks);
                changes.insert(
                    PathBuf::from(path),
                    FileChange::Update {
                        unified_diff,
                        move_path: move_path.map(PathBuf::from),
                    },
                );
            }
        }
    }
    (!changes.is_empty()).then_some(changes)
}

fn unified_diff_from_apply_patch_chunks(
    source_path: &str,
    target_path: &str,
    chunks: &[UpdateFileChunk],
) -> String {
    let mut diff = String::new();
    diff.push_str(APPLY_PATCH_DIFF_OLD_PATH_PREFIX);
    diff.push_str(source_path);
    diff.push('\n');
    diff.push_str(APPLY_PATCH_DIFF_NEW_PATH_PREFIX);
    diff.push_str(target_path);
    diff.push('\n');
    let mut old_start = 1;
    let mut new_start = 1;
    for chunk in chunks {
        let old_len = chunk.old_lines.len();
        let new_len = chunk.new_lines.len();
        let _ = write!(
            diff,
            "{APPLY_PATCH_DIFF_HUNK_PREFIX} -{} +{} {APPLY_PATCH_DIFF_HUNK_PREFIX}",
            apply_patch_range_label(old_start, old_len),
            apply_patch_range_label(new_start, new_len)
        );
        if let Some(context) = chunk.change_context.as_deref() {
            diff.push(' ');
            diff.push_str(context);
        }
        diff.push('\n');
        for line in &chunk.old_lines {
            diff.push('-');
            diff.push_str(line);
            diff.push('\n');
        }
        for line in &chunk.new_lines {
            diff.push('+');
            diff.push_str(line);
            diff.push('\n');
        }
        old_start += old_len.max(1);
        new_start += new_len.max(1);
    }
    diff
}

fn apply_patch_range_label(start: usize, len: usize) -> String {
    if len == 1 {
        start.to_string()
    } else {
        format!("{start},{len}")
    }
}
