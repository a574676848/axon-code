//! apply_patch 补丁文本解析器。
//!
//! 对齐 Codex `apply-patch` 的上下文 hunk 文本格式，将补丁文本解析为结构化
//! hunk 列表。本模块只负责解析与校验，不触碰文件系统；真正的应用逻辑在
//! [`crate::patch_apply`]。
//!
//! 官方 Lark 语法（与 Codex 对齐）：
//!
//! ```text
//! start: begin_patch environment_id? hunk+ end_patch
//! begin_patch: "*** Begin Patch" LF
//! environment_id: "*** Environment ID: " filename LF
//! end_patch: "*** End Patch" LF?
//!
//! hunk: add_hunk | delete_hunk | update_hunk
//! add_hunk: "*** Add File: " filename LF add_line+
//! delete_hunk: "*** Delete File: " filename LF
//! update_hunk: "*** Update File: " filename LF change_move? change?
//! filename: /(.+)/
//! add_line: "+" /(.+)/ LF -> line
//!
//! change_move: "*** Move to: " filename LF
//! change: (change_context | change_line)+ eof_line?
//! change_context: ("@@" | "@@ " /(.+)/) LF
//! change_line: ("+" | "-" | " ") /(.+)/ LF
//! eof_line: "*** End of File" LF
//! ```
//!
//! 解析器比严格语法略宽松：容忍 BOM、补丁前后说明文字、Markdown code fence、
//! heredoc 包裹及 hunk 之间的空行，但仍要求唯一完整的 patch envelope。

/// 补丁起始标记。
pub(crate) const BEGIN_PATCH_MARKER: &str = "*** Begin Patch";
/// 环境 ID 前缀标记。
pub(crate) const ENVIRONMENT_ID_MARKER: &str = "*** Environment ID: ";
/// 补丁结束标记。
pub(crate) const END_PATCH_MARKER: &str = "*** End Patch";
/// 新增文件标记。
pub(crate) const ADD_FILE_MARKER: &str = "*** Add File: ";
/// 删除文件标记。
pub(crate) const DELETE_FILE_MARKER: &str = "*** Delete File: ";
/// 更新文件标记。
pub(crate) const UPDATE_FILE_MARKER: &str = "*** Update File: ";
/// 移动文件标记。
pub(crate) const MOVE_TO_MARKER: &str = "*** Move to: ";
/// 文件末尾标记。
pub(crate) const EOF_MARKER: &str = "*** End of File";
/// 带描述的上下文标记。
pub(crate) const CHANGE_CONTEXT_MARKER: &str = "@@ ";
/// 空上下文标记。
pub(crate) const EMPTY_CHANGE_CONTEXT_MARKER: &str = "@@";

/// 补丁解析错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchParseError {
    /// 补丁整体边界非法（缺少 Begin/End Patch 标记等）。
    InvalidPatch(String),
    /// 单个 hunk 非法，附带补丁内行号。
    InvalidHunk {
        /// 错误描述。
        message: String,
        /// 补丁文本中的行号（从 1 开始）。
        line_number: usize,
    },
}

impl core::fmt::Display for PatchParseError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidPatch(message) => write!(formatter, "invalid patch: {message}"),
            Self::InvalidHunk {
                message,
                line_number,
            } => write!(formatter, "invalid hunk at line {line_number}, {message}"),
        }
    }
}

impl std::error::Error for PatchParseError {}

/// 单个补丁 hunk。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchHunk {
    /// 新增文件。
    AddFile {
        /// 目标路径（保留补丁原始拼写）。
        path: String,
        /// 文件完整内容（每行以 `\n` 结尾）。
        contents: String,
    },
    /// 删除文件。
    DeleteFile {
        /// 目标路径。
        path: String,
    },
    /// 更新文件，可选改名。
    UpdateFile {
        /// 源路径。
        path: String,
        /// 改名目标路径；存在时表示同时移动文件。
        move_path: Option<String>,
        /// 有序的修改块；后一块的上下文必须出现在前一块之后。
        chunks: Vec<UpdateFileChunk>,
    },
}

impl PatchHunk {
    /// 返回该 hunk 影响的路径；改名 hunk 返回移动目标路径。
    #[must_use]
    pub fn path(&self) -> &str {
        match self {
            Self::AddFile { path, .. } | Self::DeleteFile { path } => path,
            Self::UpdateFile {
                path, move_path, ..
            } => move_path.as_deref().unwrap_or(path),
        }
    }

    /// 返回更新 hunk 的源路径；新增/删除 hunk 返回自身路径。
    #[must_use]
    pub fn source_path(&self) -> &str {
        match self {
            Self::AddFile { path, .. }
            | Self::DeleteFile { path }
            | Self::UpdateFile { path, .. } => path,
        }
    }
}

/// 更新文件的单个修改块。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateFileChunk {
    /// 用于缩小定位范围的单行上下文（通常是类/方法/函数定义）。
    pub change_context: Option<String>,
    /// 被替换的连续旧行；必须出现在 `change_context` 之后。
    pub old_lines: Vec<String>,
    /// 替换后的新行。
    pub new_lines: Vec<String>,
    /// 为 true 时 `old_lines` 必须出现在源文件末尾（容忍尾随换行差异）。
    pub is_end_of_file: bool,
}

/// 解析模式。
enum ParseMode {
    /// 严格解析补丁文本（仅测试驱动覆盖）。
    #[cfg(test)]
    Strict,
    /// 宽松模式：剥离 gpt-4.1 误加的 heredoc 包裹。
    Lenient,
}

/// 将补丁文本解析为 hunk 列表（默认宽松模式）。
///
/// # Errors
/// 当补丁边界或任一 hunk 非法时返回 [`PatchParseError`]。
pub fn parse_patch(patch: &str) -> Result<Vec<PatchHunk>, PatchParseError> {
    parse_patch_text(patch, &ParseMode::Lenient)
}

fn parse_patch_text(patch: &str, mode: &ParseMode) -> Result<Vec<PatchHunk>, PatchParseError> {
    let lines: Vec<&str> = patch
        .trim_start_matches('\u{feff}')
        .trim()
        .lines()
        .collect();
    let (_, hunk_lines) = match mode {
        #[cfg(test)]
        ParseMode::Strict => check_patch_boundaries_strict(&lines)?,
        ParseMode::Lenient => check_patch_boundaries_lenient(&lines)?,
    };

    let (mut remaining_lines, mut line_number) = parse_environment_id_preamble(hunk_lines)?;
    let mut hunks: Vec<PatchHunk> = Vec::new();
    while !remaining_lines.is_empty() {
        if remaining_lines[0].trim().is_empty() {
            remaining_lines = &remaining_lines[1..];
            line_number += 1;
            continue;
        }
        let (hunk, consumed) = parse_one_hunk(remaining_lines, line_number)?;
        hunks.push(hunk);
        line_number += consumed;
        remaining_lines = &remaining_lines[consumed..];
    }
    Ok(hunks)
}

/// 严格模式解析补丁文本（不剥离 heredoc 包裹）。
///
/// # Errors
/// 当补丁边界或任一 hunk 非法时返回 [`PatchParseError`]。
#[cfg(test)]
pub(crate) fn parse_patch_strict(patch: &str) -> Result<Vec<PatchHunk>, PatchParseError> {
    parse_patch_text(patch, &ParseMode::Strict)
}

fn parse_environment_id_preamble<'a>(
    hunk_lines: &'a [&'a str],
) -> Result<(&'a [&'a str], usize), PatchParseError> {
    let Some(first_line) = hunk_lines.first() else {
        return Ok((hunk_lines, 2));
    };
    let Some(environment_id) = first_line.trim_start().strip_prefix(ENVIRONMENT_ID_MARKER) else {
        return Ok((hunk_lines, 2));
    };
    if environment_id.trim().is_empty() {
        return Err(PatchParseError::InvalidPatch(
            "apply_patch environment_id cannot be empty".to_owned(),
        ));
    }
    Ok((&hunk_lines[1..], 3))
}

/// 严格校验补丁首尾标记，返回 (整段行, hunk 行)。
fn check_patch_boundaries_strict<'a>(
    lines: &'a [&'a str],
) -> Result<(&'a [&'a str], &'a [&'a str]), PatchParseError> {
    let (first_line, last_line) = match lines {
        [] => (None, None),
        [first] => (Some(first), Some(first)),
        [first, .., last] => (Some(first), Some(last)),
    };
    check_start_and_end_lines_strict(first_line, last_line)?;
    Ok((lines, &lines[1..lines.len() - 1]))
}

/// 宽松模式：若严格校验失败且首行是 heredoc 起始、尾行以 EOF 结束，则剥离包裹后重试。
fn check_patch_boundaries_lenient<'a>(
    original_lines: &'a [&'a str],
) -> Result<(&'a [&'a str], &'a [&'a str]), PatchParseError> {
    let begin_lines = marker_line_indexes(original_lines, BEGIN_PATCH_MARKER);
    let end_lines = marker_line_indexes(original_lines, END_PATCH_MARKER);
    match (begin_lines.as_slice(), end_lines.as_slice()) {
        ([begin], [end]) if begin < end => {
            check_patch_boundaries_strict(&original_lines[*begin..=*end])
        }
        ([], _) | ([_], []) => check_patch_boundaries_strict(original_lines),
        _ => Err(PatchParseError::InvalidPatch(
            "The patch must contain exactly one complete '*** Begin Patch' / '*** End Patch' envelope"
                .to_owned(),
        )),
    }
}

fn marker_line_indexes(lines: &[&str], marker: &str) -> Vec<usize> {
    lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| (trim_patch_marker(line) == marker).then_some(index))
        .collect()
}

fn trim_patch_marker(line: &str) -> &str {
    line.trim().trim_start_matches('\u{feff}')
}

fn check_start_and_end_lines_strict(
    first_line: Option<&&str>,
    last_line: Option<&&str>,
) -> Result<(), PatchParseError> {
    let first_line = first_line.map(|line| trim_patch_marker(line));
    let last_line = last_line.map(|line| trim_patch_marker(line));

    match (first_line, last_line) {
        (Some(first), Some(last)) if first == BEGIN_PATCH_MARKER && last == END_PATCH_MARKER => {
            Ok(())
        }
        (Some(first), _) if first != BEGIN_PATCH_MARKER => Err(PatchParseError::InvalidPatch(
            "The first line of the patch must be '*** Begin Patch'".to_owned(),
        )),
        _ => Err(PatchParseError::InvalidPatch(
            "The last line of the patch must be '*** End Patch'".to_owned(),
        )),
    }
}

/// 从行序列开头解析单个 hunk，返回 (hunk, 已消费行数)。
fn parse_one_hunk(
    lines: &[&str],
    line_number: usize,
) -> Result<(PatchHunk, usize), PatchParseError> {
    let first_line = lines[0].trim();
    if let Some(path) = first_line.strip_prefix(ADD_FILE_MARKER) {
        return Ok(parse_add_file_hunk(path, &lines[1..]));
    } else if let Some(path) = first_line.strip_prefix(DELETE_FILE_MARKER) {
        return Ok((
            PatchHunk::DeleteFile {
                path: path.to_owned(),
            },
            1,
        ));
    } else if let Some(path) = first_line.strip_prefix(UPDATE_FILE_MARKER) {
        return parse_update_file_hunk(path, lines, line_number);
    }

    Err(PatchParseError::InvalidHunk {
        message: format!(
            "'{first_line}' is not a valid hunk header. Valid hunk headers: \
             '*** Add File: {{path}}', '*** Delete File: {{path}}', '*** Update File: {{path}}'"
        ),
        line_number,
    })
}

fn parse_add_file_hunk(path: &str, body: &[&str]) -> (PatchHunk, usize) {
    let mut contents = String::new();
    let mut parsed_lines = 1;
    for add_line in body {
        if let Some(line_to_add) = add_line.strip_prefix('+') {
            contents.push_str(line_to_add);
            contents.push('\n');
            parsed_lines += 1;
        } else {
            break;
        }
    }
    (
        PatchHunk::AddFile {
            path: path.to_owned(),
            contents,
        },
        parsed_lines,
    )
}

fn parse_update_file_hunk(
    path: &str,
    lines: &[&str],
    line_number: usize,
) -> Result<(PatchHunk, usize), PatchParseError> {
    let mut remaining_lines = &lines[1..];
    let mut parsed_lines = 1;
    let move_path = remaining_lines
        .first()
        .and_then(|line| line.strip_prefix(MOVE_TO_MARKER));

    if move_path.is_some() {
        remaining_lines = &remaining_lines[1..];
        parsed_lines += 1;
    }

    let mut chunks = Vec::new();
    while !remaining_lines.is_empty() {
        if remaining_lines[0].trim().is_empty() {
            parsed_lines += 1;
            remaining_lines = &remaining_lines[1..];
            continue;
        }

        if remaining_lines[0].starts_with('*') {
            break;
        }

        let (chunk, chunk_lines) = parse_update_file_chunk(
            remaining_lines,
            line_number + parsed_lines,
            chunks.is_empty(),
        )?;
        chunks.push(chunk);
        parsed_lines += chunk_lines;
        remaining_lines = &remaining_lines[chunk_lines..];
    }

    if chunks.is_empty() {
        return Err(PatchParseError::InvalidHunk {
            message: format!("Update file hunk for path '{path}' is empty"),
            line_number,
        });
    }

    Ok((
        PatchHunk::UpdateFile {
            path: path.to_owned(),
            move_path: move_path.map(ToOwned::to_owned),
            chunks,
        },
        parsed_lines,
    ))
}

fn parse_update_file_chunk(
    lines: &[&str],
    line_number: usize,
    allow_missing_context: bool,
) -> Result<(UpdateFileChunk, usize), PatchParseError> {
    if lines.is_empty() {
        return Err(PatchParseError::InvalidHunk {
            message: "Update hunk does not contain any lines".to_owned(),
            line_number,
        });
    }
    let (change_context, start_index) = if lines[0] == EMPTY_CHANGE_CONTEXT_MARKER {
        (None, 1)
    } else if let Some(context) = lines[0].strip_prefix(CHANGE_CONTEXT_MARKER) {
        (Some(context.to_owned()), 1)
    } else {
        if !allow_missing_context {
            return Err(PatchParseError::InvalidHunk {
                message: format!(
                    "Expected update hunk to start with a @@ context marker, got: '{}'",
                    lines[0]
                ),
                line_number,
            });
        }
        (None, 0)
    };
    if start_index >= lines.len() {
        return Err(PatchParseError::InvalidHunk {
            message: "Update hunk does not contain any lines".to_owned(),
            line_number: line_number + 1,
        });
    }
    let mut chunk = UpdateFileChunk {
        change_context,
        old_lines: Vec::new(),
        new_lines: Vec::new(),
        is_end_of_file: false,
    };
    let mut parsed_lines = 0;
    for line in &lines[start_index..] {
        match *line {
            EOF_MARKER => {
                if parsed_lines == 0 {
                    return Err(PatchParseError::InvalidHunk {
                        message: "Update hunk does not contain any lines".to_owned(),
                        line_number: line_number + 1,
                    });
                }
                chunk.is_end_of_file = true;
                parsed_lines += 1;
                break;
            }
            line_contents => match line_contents.chars().next() {
                None => {
                    chunk.old_lines.push(String::new());
                    chunk.new_lines.push(String::new());
                }
                Some(' ') => {
                    chunk.old_lines.push(line_contents[1..].to_owned());
                    chunk.new_lines.push(line_contents[1..].to_owned());
                }
                Some('+') => {
                    chunk.new_lines.push(line_contents[1..].to_owned());
                }
                Some('-') => {
                    chunk.old_lines.push(line_contents[1..].to_owned());
                }
                _ => {
                    if parsed_lines == 0 {
                        return Err(PatchParseError::InvalidHunk {
                            message: format!(
                                "Unexpected line found in update hunk: '{line_contents}'. \
                                 Every line should start with ' ' (context line), \
                                 '+' (added line), or '-' (removed line); \
                                 将 Python、Rust 或 shell 命令移出 PathDiff，改用 builtin.shell_command"
                            ),
                            line_number: line_number + 1,
                        });
                    }
                    break;
                }
            },
        }
        if *line != EOF_MARKER {
            parsed_lines += 1;
        }
    }

    Ok((chunk, parsed_lines + start_index))
}

#[cfg(test)]
#[path = "patch_parser/tests.rs"]
mod tests;
