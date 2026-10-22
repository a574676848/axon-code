use std::path::{Path, PathBuf};

use axon_agent_runtime::{
    GlobToolRuntime, LspBackgroundRuntime, ReadOnlyFsToolRuntime,
    agent_run_glob_tool_input_from_provider_args, agent_run_glob_tool_receipt,
    agent_run_read_file_tool_input_from_provider_args, agent_run_read_file_tool_receipt,
};
use serde_json::json;

use crate::tool_orchestrator::{
    ToolOrchestratorContext, ToolReceipt, failed_tool_input_receipt,
    failed_tool_receipt_from_axon_error, tool_receipt_from_runtime,
};

const APPLY_DIFF_CONTEXT_RADIUS: usize = 3;

pub(crate) fn execute_read_file(
    tool_call: &axon_agent_runtime::ProviderToolCall,
    context: &ToolOrchestratorContext,
) -> ToolReceipt {
    let canonical_input = tool_call.canonical_input();
    let mut input =
        match agent_run_read_file_tool_input_from_provider_args(&context.cwd, &canonical_input) {
            Ok(input) => input,
            Err(message) => return failed_tool_input_receipt(tool_call, message),
        };
    input.snapshot_session = Some(context.workspace_files.clone());
    let receipt = match ReadOnlyFsToolRuntime.read_file(input) {
        Ok(receipt) => receipt,
        Err(error) => return failed_tool_receipt_from_axon_error(tool_call, error),
    };
    let lsp_background =
        LspBackgroundRuntime::schedule(&context.cwd, [PathBuf::from(&receipt.path)]);
    let mut tool_receipt =
        tool_receipt_from_runtime(agent_run_read_file_tool_receipt(tool_call, &receipt));
    if let Some(metadata) = tool_receipt.metadata.as_mut()
        && let Some(object) = metadata.as_object_mut()
    {
        object.insert(
            "lspBackground".to_owned(),
            serde_json::to_value(lsp_background).unwrap_or_else(|_| {
                json!({
                    "status": "unavailable",
                    "files": [],
                })
            }),
        );
    }
    tool_receipt
}

pub(crate) fn execute_glob(
    tool_call: &axon_agent_runtime::ProviderToolCall,
    context: &ToolOrchestratorContext,
) -> ToolReceipt {
    let canonical_input = tool_call.canonical_input();
    let mut input =
        match agent_run_glob_tool_input_from_provider_args(&context.cwd, &canonical_input) {
            Ok(input) => input,
            Err(message) => return failed_tool_input_receipt(tool_call, message),
        };
    input.cancel_token = context.cancel_token.clone();
    let receipt = match GlobToolRuntime.execute(input) {
        Ok(receipt) => receipt,
        Err(error) => return failed_tool_receipt_from_axon_error(tool_call, error),
    };
    tool_receipt_from_runtime(agent_run_glob_tool_receipt(tool_call, &receipt))
}

pub(crate) fn resolve_tool_path(path: &str) -> Result<PathBuf, String> {
    let normalized = path.replace('\\', "/");
    let path = PathBuf::from(normalized);
    if path.as_os_str().is_empty() {
        return Err("工具 path 不能为空".to_owned());
    }
    Ok(path)
}

pub(crate) fn current_file_content(
    root: &Path,
    relative_path: &Path,
) -> Result<Option<String>, String> {
    let target = root.join(relative_path);
    match std::fs::read_to_string(&target) {
        Ok(content) => Ok(Some(content)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("apply_diff 读取当前文件失败：{error}")),
    }
}

pub(crate) fn apply_diff_file_changes(
    path: &str,
    previous_content: Option<&str>,
    next_content: &str,
) -> serde_json::Value {
    let (operation, diff) = match previous_content {
        Some(previous) => (
            "updated",
            unified_diff_for_text_write(path, previous, next_content),
        ),
        None => ("added", next_content.to_owned()),
    };
    json!([{
        "path": path,
        "operation": operation,
        "diff": diff,
    }])
}

fn unified_diff_for_text_write(path: &str, previous_content: &str, next_content: &str) -> String {
    let old_lines = split_diff_lines(previous_content);
    let new_lines = split_diff_lines(next_content);

    let prefix_len = common_prefix_len(&old_lines, &new_lines);
    let suffix_len = common_suffix_len(&old_lines, &new_lines, prefix_len);
    let old_changed_end = old_lines.len().saturating_sub(suffix_len);
    let new_changed_end = new_lines.len().saturating_sub(suffix_len);
    let old_context_start = prefix_len.saturating_sub(APPLY_DIFF_CONTEXT_RADIUS);
    let new_context_start = prefix_len.saturating_sub(APPLY_DIFF_CONTEXT_RADIUS);
    let old_context_end = (old_changed_end + APPLY_DIFF_CONTEXT_RADIUS).min(old_lines.len());
    let new_context_end = (new_changed_end + APPLY_DIFF_CONTEXT_RADIUS).min(new_lines.len());

    let mut diff = format!(
        "--- {path}\n+++ {path}\n@@ -{},{} +{},{} @@\n",
        old_context_start + 1,
        old_context_end.saturating_sub(old_context_start),
        new_context_start + 1,
        new_context_end.saturating_sub(new_context_start)
    );
    for line in &old_lines[old_context_start..prefix_len] {
        diff.push(' ');
        diff.push_str(line);
        diff.push('\n');
    }
    for line in &old_lines[prefix_len..old_changed_end] {
        diff.push('-');
        diff.push_str(line);
        diff.push('\n');
    }
    for line in &new_lines[prefix_len..new_changed_end] {
        diff.push('+');
        diff.push_str(line);
        diff.push('\n');
    }
    for line in &old_lines[old_changed_end..old_context_end] {
        diff.push(' ');
        diff.push_str(line);
        diff.push('\n');
    }
    diff
}

fn split_diff_lines(content: &str) -> Vec<&str> {
    if content.is_empty() {
        return Vec::new();
    }
    content
        .strip_suffix('\n')
        .unwrap_or(content)
        .split('\n')
        .collect()
}

fn common_prefix_len(old_lines: &[&str], new_lines: &[&str]) -> usize {
    old_lines
        .iter()
        .zip(new_lines)
        .take_while(|(old, new)| old == new)
        .count()
}

fn common_suffix_len(old_lines: &[&str], new_lines: &[&str], prefix_len: usize) -> usize {
    old_lines
        .iter()
        .rev()
        .zip(new_lines.iter().rev())
        .take_while(|(old, new)| old == new)
        .take(old_lines.len().saturating_sub(prefix_len))
        .take(new_lines.len().saturating_sub(prefix_len))
        .count()
}

#[cfg(test)]
#[path = "tests/fs_tests.rs"]
mod tests;
