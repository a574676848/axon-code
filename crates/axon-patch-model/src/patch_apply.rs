//! apply_patch 补丁应用引擎。
//!
//! 在已解析的 [`PatchHunk`] 列表上，结合工作区当前文件内容计算出每个文件的
//! 目标内容。本模块是同步纯函数：输入「补丁 hunks + 现有文件内容」，输出
//! 「结构化文件变更计划」，不直接写盘；调用方在既有审批与权限门禁后完成落盘。
//!
//! 对齐 Codex `apply-patch` 的 `MaybeApplyPatchVerified` 验证模式，提供：
//! - 预验证层：在应用前校验补丁与文件系统的一致性
//! - 增强错误分类：记录匹配级别与诊断提示

use std::collections::{HashMap, HashSet};

use crate::patch_parser::{PatchHunk, UpdateFileChunk};
use crate::patch_seek::seek_sequence;

/// 匹配级别：用于诊断和重试策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MatchLevel {
    /// 未尝试匹配
    None,
    /// 第1级：精确匹配（字节级一致）
    Exact,
    /// 第2级：忽略尾随空白匹配
    TrimEnd,
    /// 第3级：忽略首尾空白匹配
    Trim,
    /// 第4级：Unicode 标点归一化匹配
    UnicodeNormalize,
}

/// 应用补丁时发生的错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchApplyError {
    /// 补丁未包含任何 hunk。
    EmptyPatch,
    /// 更新/删除目标文件内容缺失（调用方未提供）。
    MissingFileContent {
        /// 缺失内容的路径。
        path: String,
    },
    /// 在目标文件中找不到上下文行。
    ContextNotFound {
        /// 目标路径。
        path: String,
        /// 未能定位的上下文。
        context: String,
        /// 尝试过的匹配级别。
        attempted_levels: Vec<MatchLevel>,
    },
    /// 在目标文件中找不到待替换的旧行。
    OldLinesNotFound {
        /// 目标路径。
        path: String,
        /// 未能定位的旧行联合文本。
        old_lines: String,
        /// 尝试过的匹配级别。
        attempted_levels: Vec<MatchLevel>,
        /// 文件中相似行（用于诊断提示）。
        similar_lines: Vec<String>,
    },
    /// 隐式调用：直接传递补丁文本而未通过 apply_patch 命令。
    ImplicitInvocation,
    /// 文件读取 I/O 错误。
    IoError {
        /// 路径。
        path: String,
        /// 错误消息。
        message: String,
    },
    /// 补丁文本解析错误。
    ParseError(crate::patch_parser::PatchParseError),
    /// 同一 PathDiff 批次重复占用文件路径。
    DuplicatePath {
        /// 重复的源路径或目标路径。
        path: String,
    },
}

impl core::fmt::Display for PatchApplyError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::EmptyPatch => write!(formatter, "No files were modified."),
            Self::MissingFileContent { path } => {
                write!(formatter, "Failed to read file to update {path}")
            }
            Self::ContextNotFound {
                path,
                context,
                attempted_levels,
            } => {
                write!(formatter, "Failed to find context '{context}' in {path}")?;
                if !attempted_levels.is_empty() {
                    let levels: Vec<&str> = attempted_levels
                        .iter()
                        .map(|l| match l {
                            MatchLevel::None => "none",
                            MatchLevel::Exact => "exact",
                            MatchLevel::TrimEnd => "trim-end",
                            MatchLevel::Trim => "trim",
                            MatchLevel::UnicodeNormalize => "unicode-norm",
                        })
                        .collect();
                    write!(formatter, " (attempted: {})", levels.join(", "))?;
                }
                Ok(())
            }
            Self::OldLinesNotFound {
                path,
                old_lines,
                attempted_levels,
                similar_lines,
            } => {
                write!(
                    formatter,
                    "Failed to find expected lines in {path}:\n{old_lines}"
                )?;
                if !attempted_levels.is_empty() {
                    let levels: Vec<&str> = attempted_levels
                        .iter()
                        .map(|l| match l {
                            MatchLevel::None => "none",
                            MatchLevel::Exact => "exact",
                            MatchLevel::TrimEnd => "trim-end",
                            MatchLevel::Trim => "trim",
                            MatchLevel::UnicodeNormalize => "unicode-norm",
                        })
                        .collect();
                    write!(formatter, " (attempted: {})", levels.join(", "))?;
                }
                if !similar_lines.is_empty() {
                    write!(
                        formatter,
                        "\nSimilar lines found:\n{}",
                        similar_lines.join("\n")
                    )?;
                }
                Ok(())
            }
            Self::ImplicitInvocation => {
                write!(
                    formatter,
                    "apply_patch called implicitly without using the apply_patch command"
                )
            }
            Self::IoError { path, message } => {
                write!(formatter, "I/O error on {path}: {message}")
            }
            Self::ParseError(e) => {
                write!(formatter, "patch parse error: {e}")
            }
            Self::DuplicatePath { path } => {
                write!(formatter, "PathDiff 同一批次重复修改文件：{path}")
            }
        }
    }
}

impl std::error::Error for PatchApplyError {}

impl From<crate::patch_parser::PatchParseError> for PatchApplyError {
    fn from(e: crate::patch_parser::PatchParseError) -> Self {
        Self::ParseError(e)
    }
}

/// 单个文件的变更类型。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchFileChange {
    /// 新增文件，写入完整内容。
    Add {
        /// 新文件完整内容。
        content: String,
    },
    /// 删除文件。
    Delete {
        /// 被删除文件的原内容（若调用方提供）。
        previous_content: Option<String>,
    },
    /// 更新文件，可选改名。
    Update {
        /// 改名目标路径；存在时表示需要移动文件。
        move_path: Option<String>,
        /// 应用后的完整内容。
        new_content: String,
    },
}

/// 单个文件的变更项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchFileOperation {
    /// 目标路径（保留补丁原始拼写）。
    pub path: String,
    /// 变更内容。
    pub change: PatchFileChange,
}

/// 补丁应用计划：所有 hunk 计算出的结构化文件变更，按补丁出现顺序排列。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PatchPlan {
    /// 有序文件变更。
    pub operations: Vec<PatchFileOperation>,
}

/// 工作区当前文件内容快照：路径 → 内容。
pub type WorkspaceSnapshot<'a> = &'a HashMap<String, String>;

/// 根据补丁 hunks 与当前文件内容计算文件变更计划。
///
/// `snapshot` 提供更新/删除目标文件的现有内容（键为补丁中的源路径）。
///
/// # Errors
/// 当补丁为空、路径重复、缺少目标文件内容或上下文匹配失败时返回 [`PatchApplyError`]。
pub fn compute_plan(
    hunks: &[PatchHunk],
    snapshot: WorkspaceSnapshot<'_>,
) -> Result<PatchPlan, PatchApplyError> {
    if hunks.is_empty() {
        return Err(PatchApplyError::EmptyPatch);
    }

    validate_unique_paths(hunks)?;

    let mut operations = Vec::with_capacity(hunks.len());
    for hunk in hunks {
        let operation = match hunk {
            // 命中已存在文件时按整文件重写处理：Codex apply_patch 同样允许 Add 覆盖
            // 已存在文件。走 Update 链路可复用「备份源文件 → 硬链提升 → 失败回滚」的既有
            // 事务语义，并在提交前继承原文件的行尾与 BOM。
            PatchHunk::AddFile { path, contents } => match snapshot.get(path) {
                Some(_) => PatchFileOperation {
                    path: path.clone(),
                    change: PatchFileChange::Update {
                        move_path: None,
                        new_content: contents.clone(),
                    },
                },
                None => PatchFileOperation {
                    path: path.clone(),
                    change: PatchFileChange::Add {
                        content: contents.clone(),
                    },
                },
            },
            PatchHunk::DeleteFile { path } => PatchFileOperation {
                path: path.clone(),
                change: PatchFileChange::Delete {
                    previous_content: snapshot.get(path).cloned(),
                },
            },
            PatchHunk::UpdateFile {
                path,
                move_path,
                chunks,
            } => {
                let original = snapshot
                    .get(path)
                    .ok_or_else(|| PatchApplyError::MissingFileContent { path: path.clone() })?;
                let new_content = derive_new_contents(path, original, chunks)?;
                PatchFileOperation {
                    path: path.clone(),
                    change: PatchFileChange::Update {
                        move_path: move_path.clone(),
                        new_content,
                    },
                }
            }
        };
        operations.push(operation);
    }

    Ok(PatchPlan { operations })
}

fn validate_unique_paths(hunks: &[PatchHunk]) -> Result<(), PatchApplyError> {
    let mut paths = HashSet::new();
    for hunk in hunks {
        let source_path = hunk.source_path();
        if !paths.insert(source_path) {
            return Err(PatchApplyError::DuplicatePath {
                path: source_path.to_owned(),
            });
        }

        let target_path = hunk.path();
        if target_path != source_path && !paths.insert(target_path) {
            return Err(PatchApplyError::DuplicatePath {
                path: target_path.to_owned(),
            });
        }
    }
    Ok(())
}

/// 对单个文件应用 update chunks，返回应用后的完整内容。
fn derive_new_contents(
    path: &str,
    original_contents: &str,
    chunks: &[UpdateFileChunk],
) -> Result<String, PatchApplyError> {
    let mut original_lines: Vec<String> = original_contents.split('\n').map(String::from).collect();
    // 丢弃末尾换行带来的空元素，使行计数与标准 diff 一致。
    if original_lines.last().is_some_and(String::is_empty) {
        original_lines.pop();
    }

    let replacements = compute_replacements(&original_lines, path, chunks)?;
    let mut new_lines = apply_replacements(original_lines, &replacements);
    if !new_lines.last().is_some_and(String::is_empty) {
        new_lines.push(String::new());
    }
    Ok(new_lines.join("\n"))
}

/// 计算将 `original_lines` 转换为新内容所需的替换列表。
/// 每个替换为 `(start_index, old_len, new_lines)`。
fn compute_replacements(
    original_lines: &[String],
    path: &str,
    chunks: &[UpdateFileChunk],
) -> Result<Vec<(usize, usize, Vec<String>)>, PatchApplyError> {
    match compute_replacements_ordered(original_lines, path, chunks) {
        Ok(res) => Ok(res),
        Err(err) if chunks.len() > 1 => {
            if let Some(sorted) = try_sort_chunks_by_position(original_lines, chunks)
                && let Ok(res) = compute_replacements_ordered(original_lines, path, &sorted)
            {
                return Ok(res);
            }
            Err(err)
        }
        Err(err) => Err(err),
    }
}

fn try_sort_chunks_by_position(
    original_lines: &[String],
    chunks: &[UpdateFileChunk],
) -> Option<Vec<UpdateFileChunk>> {
    let mut positioned: Vec<(usize, usize, UpdateFileChunk)> = Vec::with_capacity(chunks.len());
    for chunk in chunks {
        let pattern = if chunk.old_lines.is_empty() {
            if let Some(ctx) = &chunk.change_context {
                seek_sequence(original_lines, std::slice::from_ref(ctx), 0, false)?
            } else {
                original_lines.len()
            }
        } else {
            let start = if let Some(ctx) = &chunk.change_context {
                seek_sequence(original_lines, std::slice::from_ref(ctx), 0, false)?
            } else {
                0
            };
            seek_sequence(
                original_lines,
                &chunk.old_lines,
                start,
                chunk.is_end_of_file,
            )?
        };
        let len = chunk.old_lines.len();
        positioned.push((pattern, len, chunk.clone()));
    }
    positioned.sort_by_key(|(start, _, _)| *start);
    for i in 1..positioned.len() {
        let prev_end = positioned[i - 1].0 + positioned[i - 1].1;
        if positioned[i].0 < prev_end {
            return None;
        }
    }
    Some(positioned.into_iter().map(|(_, _, chunk)| chunk).collect())
}

fn compute_replacements_ordered(
    original_lines: &[String],
    path: &str,
    chunks: &[UpdateFileChunk],
) -> Result<Vec<(usize, usize, Vec<String>)>, PatchApplyError> {
    let mut replacements: Vec<(usize, usize, Vec<String>)> = Vec::new();
    let mut line_index: usize = 0;

    for chunk in chunks {
        if let Some(ctx_line) = &chunk.change_context {
            if let Some(idx) = seek_sequence(
                original_lines,
                std::slice::from_ref(ctx_line),
                line_index,
                false,
            ) {
                line_index = idx + 1;
            } else {
                return Err(PatchApplyError::ContextNotFound {
                    path: path.to_owned(),
                    context: ctx_line.clone(),
                    attempted_levels: vec![MatchLevel::Exact],
                });
            }
        }

        if chunk.old_lines.is_empty() {
            // 纯新增：插入到文件末尾（或末尾空行之前）。
            let insertion_idx = if original_lines.last().is_some_and(String::is_empty) {
                original_lines.len() - 1
            } else {
                original_lines.len()
            };
            replacements.push((insertion_idx, 0, chunk.new_lines.clone()));
            continue;
        }

        let mut pattern: &[String] = &chunk.old_lines;
        let mut found = seek_sequence(original_lines, pattern, line_index, chunk.is_end_of_file);
        let mut new_slice: &[String] = &chunk.new_lines;

        // 旧行末尾的空串代表文件终止换行，原文件已剥离；定位失败时去掉再试。
        if found.is_none() && pattern.last().is_some_and(String::is_empty) {
            pattern = &pattern[..pattern.len() - 1];
            if new_slice.last().is_some_and(String::is_empty) {
                new_slice = &new_slice[..new_slice.len() - 1];
            }
            found = seek_sequence(original_lines, pattern, line_index, chunk.is_end_of_file);
        }

        if let Some(start_idx) = found {
            replacements.push((start_idx, pattern.len(), new_slice.to_vec()));
            line_index = start_idx + pattern.len();
        } else {
            // 收集相似行用于诊断
            let similar_lines = collect_similar_lines(original_lines, &chunk.old_lines);
            return Err(PatchApplyError::OldLinesNotFound {
                path: path.to_owned(),
                old_lines: chunk.old_lines.join("\n"),
                attempted_levels: vec![MatchLevel::Exact],
                similar_lines,
            });
        }
    }

    replacements.sort_by_key(|(index, _, _)| *index);
    Ok(replacements)
}

/// 收集与目标行相似的行，用于诊断提示。
fn collect_similar_lines(original_lines: &[String], target_lines: &[String]) -> Vec<String> {
    if target_lines.is_empty() || original_lines.is_empty() {
        return Vec::new();
    }

    // 取目标的第一行作为主要匹配目标
    let primary_target = &target_lines[0];
    let threshold = if primary_target.len() > 10 { 3 } else { 1 };

    original_lines
        .iter()
        .filter(|line| {
            // 过滤条件：长度相近或包含相同关键词
            line.len().saturating_sub(primary_target.len()) <= threshold * 2
                || levenshtein_similarity(line, primary_target) >= 0.6
        })
        .cloned()
        .collect()
}

/// 计算两个字符串的 Levenshtein 相似度（0.0 ~ 1.0）。
fn levenshtein_similarity(a: &str, b: &str) -> f32 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }

    let len_a = a.len();
    let len_b = b.len();

    // 动态规划计算 Levenshtein 距离
    let mut matrix = vec![vec![0usize; len_b + 1]; len_a + 1];

    for (i, row) in matrix.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, value) in matrix[0].iter_mut().enumerate() {
        *value = j;
    }

    for i in 1..=len_a {
        for j in 1..=len_b {
            let cost = usize::from(a.as_bytes()[i - 1] != b.as_bytes()[j - 1]);
            matrix[i][j] = (matrix[i - 1][j] + 1)
                .min(matrix[i][j - 1] + 1)
                .min(matrix[i - 1][j - 1] + cost);
        }
    }

    let distance = matrix[len_a][len_b];
    let max_len = len_a.max(len_b);
    1.0 - (distance as f32 / max_len as f32)
}

/// 将替换列表应用到行序列，返回修改后的行。
fn apply_replacements(
    mut lines: Vec<String>,
    replacements: &[(usize, usize, Vec<String>)],
) -> Vec<String> {
    // 必须降序应用，避免前面的替换移动后面替换的下标。
    for (start_idx, old_len, new_segment) in replacements.iter().rev() {
        let start_idx = *start_idx;
        let old_len = *old_len;

        for _ in 0..old_len {
            if start_idx < lines.len() {
                lines.remove(start_idx);
            }
        }

        for (offset, new_line) in new_segment.iter().enumerate() {
            lines.insert(start_idx + offset, new_line.clone());
        }
    }

    lines
}

#[cfg(test)]
#[path = "patch_apply/tests.rs"]
mod tests;
