//! apply_patch 结构化错误诊断、局部 Diff 透视与失败 Micro-read 切片生成。

use axon_patch_model::{PatchApplyError, find_best_matching_window};
use std::collections::HashMap;

/// 将 PatchApplyError 升格为包含预期 vs 实际切片对比与自愈建议的高维度诊断信息。
pub(crate) fn format_patch_apply_diagnostic(
    error: &PatchApplyError,
    snapshot: &HashMap<String, String>,
) -> String {
    match error {
        PatchApplyError::ContextNotFound { path, context, .. } => {
            let file_content = snapshot.get(path).map(String::as_str).unwrap_or_default();
            format_context_mismatch_diagnostic(path, context, file_content)
        }
        PatchApplyError::OldLinesNotFound {
            path, old_lines, ..
        } => {
            let file_content = snapshot.get(path).map(String::as_str).unwrap_or_default();
            format_old_lines_mismatch_diagnostic(path, old_lines, file_content)
        }
        PatchApplyError::DuplicatePath { path } => {
            format!(
                "[builtin.edit 执行失败: DUPLICATE_PATH]\n\
                 目标文件: {path}\n\
                 【原因定位】：同一个 PathDiff 补丁批次中多次出现了对文件 '{path}' 的操作。\n\
                 【建议动作】：请将对该文件的多次修改合并到同一个 '*** Update File: {path}' 块中，并在内部使用多个 '@@' 分段。"
            )
        }
        PatchApplyError::MissingFileContent { path } => {
            format!(
                "[builtin.edit 执行失败: FILE_NOT_FOUND]\n\
                 目标文件: {path}\n\
                 【原因定位】：工作区中未找到需要修改或删除的目标文件。\n\
                 【建议动作】：若要新建该文件，请改用 '*** Add File: {path}' 指令。"
            )
        }
        PatchApplyError::EmptyPatch => "[builtin.edit 执行失败: EMPTY_PATCH]\n\
             【原因定位】：提交的补丁中未包含任何有效的文件修改操作。"
            .to_owned(),
        other => format!("PathDiff 预检失败：{other}"),
    }
}

fn format_context_mismatch_diagnostic(
    path: &str,
    expected_context: &str,
    file_content: &str,
) -> String {
    let lines: Vec<String> = file_content.lines().map(ToOwned::to_owned).collect();
    let pattern = vec![expected_context.to_owned()];

    let mut message = format!(
        "[builtin.edit 执行失败: HUNK_CONTEXT_MISMATCH]\n\
         目标文件: {path}\n\
         【原因定位】：补丁中指定的上下文行 '@@ {expected_context}' 在目标文件中未找到匹配项。\n"
    );

    if let Some((best_idx, score)) = find_best_matching_window(&lines, &pattern, 0) {
        let actual_line = &lines[best_idx];
        let pct = (score * 100.0) as u32;
        let line_no = best_idx + 1;
        message.push_str(&format!(
            "【最相似位置】（原文件第 {line_no} 行，相似度 {pct}%）：\n\
             --- 预期上下文 (Expected)\n\
             +++ 目标文件实际内容 (Actual) [第 {line_no} 行]\n\
             - {expected_context}\n\
             + {actual_line}\n\n"
        ));

        let snippet = render_micro_read_snippet(&lines, best_idx, 4);
        message.push_str(&format!(
            "【附近只读切片 (Micro-read Snippet)】：\n\
             {snippet}\n\n\
             【建议动作】：\n\
             1. 目标行可能已被外部修改或参数签名已变动；\n\
             2. 请直接采用上方【附近只读切片】中的真实上下文行重新生成补丁，无需再次调用 read_file。"
        ));
    } else {
        message.push_str(
            "【建议动作】：目标文件中未找到相似行，请调用 builtin.read_file 重新读取该文件确认最新结构。"
        );
    }

    message
}

fn format_old_lines_mismatch_diagnostic(
    path: &str,
    expected_old_lines: &str,
    file_content: &str,
) -> String {
    let lines: Vec<String> = file_content.lines().map(ToOwned::to_owned).collect();
    let pattern: Vec<String> = expected_old_lines.lines().map(ToOwned::to_owned).collect();

    let mut message = format!(
        "[builtin.edit 执行失败: HUNK_OLD_LINES_MISMATCH]\n\
         目标文件: {path}\n\
         【原因定位】：待替换的代码块在目标文件中未找到完全匹配项。\n"
    );

    if !pattern.is_empty()
        && let Some((best_idx, score)) = find_best_matching_window(&lines, &pattern, 0)
    {
        let pct = (score * 100.0) as u32;
        let start_line = best_idx + 1;
        let end_line = (best_idx + pattern.len()).min(lines.len());
        message.push_str(&format!(
            "【最相似位置】（原文件第 {start_line}-{end_line} 行附近，平均相似度 {pct}%）：\n\
                 --- 您提交的旧代码块 (Expected)\n\
                 +++ 目标文件当前内容 (Actual)\n"
        ));
        for p in &pattern {
            message.push_str(&format!("- {p}\n"));
        }
        for actual in &lines[best_idx..end_line] {
            message.push_str(&format!("+ {actual}\n"));
        }
        message.push('\n');

        let snippet = render_micro_read_snippet(&lines, best_idx, 5);
        message.push_str(&format!(
            "【附近只读切片 (Micro-read Snippet)】：\n\
                 {snippet}\n\n\
                 【建议动作】：\n\
                 1. 请仔细核对上方 -/+ 差异（通常为缩进差异、多加了分号或变量改名）；\n\
                 2. 直接基于上方【附近只读切片】的真实内容更新待删除代码块并重新提交。"
        ));
        return message;
    }

    message.push_str(
        "【建议动作】：目标代码块偏离过大，请调用 builtin.read_file 查看目标范围最新代码后重试。",
    );
    message
}

fn render_micro_read_snippet(lines: &[String], anchor_idx: usize, radius: usize) -> String {
    let start = anchor_idx.saturating_sub(radius);
    let end = (anchor_idx + radius + 1).min(lines.len());
    let mut snippet = String::new();
    for (idx, line) in lines[start..end].iter().enumerate() {
        let line_num = start + idx + 1;
        let marker = if start + idx == anchor_idx { ">" } else { " " };
        snippet.push_str(&format!("{marker} {line_num:4} | {line}\n"));
    }
    snippet.trim_end().to_owned()
}
