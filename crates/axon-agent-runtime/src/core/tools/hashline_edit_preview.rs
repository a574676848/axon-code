//! Hashline 流式预览：把可能仍在生成的编辑输入渲染成只读 unified diff。
//!
//! 从 `hashline_edit` 拆出：预览**只读**，不写盘也不改会话状态，与「应用编辑」是两件事；
//! 两者的失败语义、输入容忍度与调用时机都不同，放在一起只会让两边都难读。

use super::*;

/// 预览时允许丢弃的尾部行数上限。
///
/// 流式输入的最后几行可能还没写完，逐行试探时只允许在末尾这么长的窗口里回退，
/// 否则会把「还没生成完」误判成「语法错误」。
const PREVIEW_MAX_TRAILING_DROPS: usize = 64;

/// 流式预览中的一个文件变更。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HashlinePreviewFile {
    /// 工具路径。
    pub path: String,
    /// 文件级操作。
    pub operation: HashlinePreviewOperation,
    /// 只读预览用的 unified diff。
    pub unified_diff: String,
}

/// 预览中的文件级操作。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HashlinePreviewOperation {
    /// 原地更新。
    Update,
    /// 删除文件。
    Delete,
    /// 移动到目标路径。
    Move {
        /// 移动目标路径。
        destination: String,
    },
}

/// 对**可能仍在流式生成**的 Hashline 输入做只读 diff 预览。
///
/// `lookup` 按工具路径返回当前文件内容；只有内容哈希与 section 标签一致时才生成预览，
/// 陈旧标签、未完成的操作、块解析失败一律跳过：预览绝不能把半截输入呈现为最终结果。
/// 预览不写盘、不改会话状态，仅用于把真实 diff 提前展示给用户。
#[must_use]
pub fn hashline_preview_files(
    input: &str,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Vec<HashlinePreviewFile> {
    let Some(sections) = parse_preview_sections(input) else {
        return Vec::new();
    };
    let mut files = Vec::new();
    for section in sections {
        let Some(content) = lookup(&section.path) else {
            continue;
        };
        if !section
            .tag
            .eq_ignore_ascii_case(&hashline_snapshot_tag(&content))
        {
            continue;
        }
        let Ok((ops, _, _)) = resolve_block_ops(&section.path, &content, section.ops) else {
            continue;
        };
        let mut text = HashlineText::parse(&content);
        let Ok((ops, _)) = normalize_edit_semantics(&text.lines, ops) else {
            continue;
        };
        if ops.is_empty() {
            continue;
        }
        let unified_diff = preview_unified_diff(&section.path, &text.lines, &ops);
        text.lines.clear();
        files.push(HashlinePreviewFile {
            path: section.path,
            operation: match section.file_operation {
                HashlineFileOperation::Update => HashlinePreviewOperation::Update,
                HashlineFileOperation::Delete => HashlinePreviewOperation::Delete,
                HashlineFileOperation::Move { destination } => {
                    HashlinePreviewOperation::Move { destination }
                }
            },
            unified_diff,
        });
    }
    files
}

/// 预览解析：逐行回退直到前缀可解析，容忍末尾未完成的操作或 body 行。
fn parse_preview_sections(input: &str) -> Option<Vec<HashlineSection>> {
    let mut candidate = input.trim_end_matches(['\r', '\n']);
    for _ in 0..PREVIEW_MAX_TRAILING_DROPS {
        if let Ok(sections) = parse_sections(candidate) {
            return Some(sections);
        }
        let index = candidate.rfind('\n')?;
        candidate = &candidate[..index];
    }
    None
}

/// 按**原始行号**渲染只读 unified diff。
///
/// 预览与正式执行的区别：这里不重排操作、不做 rebase，多个操作共存时 `+` 侧行号会带有
/// 估算偏移；越界访问一律退化为空片段，绝不在预览路径上 panic。
fn preview_unified_diff(source_path: &str, lines: &[String], ops: &[HashlineOp]) -> String {
    let mut diff = String::new();
    let _ = writeln!(diff, "--- {source_path}");
    let _ = writeln!(diff, "+++ {source_path}");
    let mut delta: isize = 0;
    for op in ops {
        match op {
            HashlineOp::Swap {
                start,
                end,
                lines: new_lines,
            } => preview_replace_hunk(&mut diff, *start, *end, lines, new_lines, &mut delta),
            HashlineOp::Delete { start, end } => {
                preview_replace_hunk(&mut diff, *start, *end, lines, &[], &mut delta);
            }
            HashlineOp::InsertBefore {
                line,
                lines: new_lines,
            } => {
                preview_insert_hunk(
                    &mut diff,
                    line.saturating_sub(1),
                    *line,
                    new_lines,
                    &mut delta,
                );
            }
            HashlineOp::InsertAfter {
                line,
                lines: new_lines,
                ..
            } => preview_insert_hunk(
                &mut diff,
                *line,
                line.saturating_add(1),
                new_lines,
                &mut delta,
            ),
            HashlineOp::InsertHead { lines: new_lines } => {
                preview_insert_hunk(&mut diff, 0, 1, new_lines, &mut delta);
            }
            HashlineOp::InsertTail { lines: new_lines } => {
                let at = lines.len();
                preview_insert_hunk(&mut diff, at, at.saturating_add(1), new_lines, &mut delta);
            }
            HashlineOp::SwapBlock { .. }
            | HashlineOp::DeleteBlock { .. }
            | HashlineOp::InsertAfterBlock { .. } => {}
        }
    }
    diff
}

fn preview_replace_hunk(
    diff: &mut String,
    start: usize,
    end: usize,
    lines: &[String],
    new_lines: &[String],
    delta: &mut isize,
) {
    let old_len = end.saturating_sub(start).saturating_add(1);
    let new_start = offset_position(start, *delta);
    let _ = writeln!(
        diff,
        "@@ -{start},{old_len} +{new_start},{} @@",
        new_lines.len()
    );
    if let Some(slice) = lines.get(start.saturating_sub(1)..end) {
        for line in slice {
            let _ = writeln!(diff, "-{line}");
        }
    }
    for line in new_lines {
        let _ = writeln!(diff, "+{line}");
    }
    *delta = delta.saturating_add(
        isize::try_from(new_lines.len())
            .unwrap_or(isize::MAX)
            .saturating_sub(isize::try_from(old_len).unwrap_or(isize::MAX)),
    );
}

fn preview_insert_hunk(
    diff: &mut String,
    before: usize,
    anchor: usize,
    new_lines: &[String],
    delta: &mut isize,
) {
    let new_start = offset_position(anchor, *delta);
    let _ = writeln!(diff, "@@ -{before},0 +{new_start},{} @@", new_lines.len());
    for line in new_lines {
        let _ = writeln!(diff, "+{line}");
    }
    *delta = delta.saturating_add(isize::try_from(new_lines.len()).unwrap_or(isize::MAX));
}

fn offset_position(position: usize, delta: isize) -> usize {
    let shifted = isize::try_from(position)
        .unwrap_or(isize::MAX)
        .saturating_add(delta);
    usize::try_from(shifted.max(1)).unwrap_or(1)
}
