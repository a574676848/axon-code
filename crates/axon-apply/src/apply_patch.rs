//! Transactional application of complete text patches inside one workspace.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use axon_patch_model::{
    PatchFileChange, PatchHunk, PatchPlan, compute_plan, inherit_text_file_format, parse_patch,
    text_change_visible_lines, text_file_line_count,
};

#[path = "apply_patch_path_validation.rs"]
mod path_validation;
use path_validation::safe_workspace_join;
#[path = "apply_patch_transaction.rs"]
mod transaction;
use transaction::materialize;
#[path = "apply_patch_diagnostic.rs"]
mod diagnostic;
use diagnostic::format_patch_apply_diagnostic;

#[cfg(test)]
#[path = "apply_patch_tests.rs"]
mod tests;

const VISIBLE_CONTEXT_RADIUS: usize = 3;

/// Error returned while parsing, validating, or applying a workspace patch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyPatchError {
    message: String,
}

impl ApplyPatchError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl core::fmt::Display for ApplyPatchError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ApplyPatchError {}

/// Stable file-level outcome for a complete patch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyPatchFileReport {
    /// Result path, relative to the normalized workspace root.
    pub path: String,
    /// Original source path for a move.
    pub source_path: Option<String>,
    /// One of `add`, `update`, `delete`, or `move`.
    pub operation: String,
    /// Renderable patch content for this file.
    pub diff: String,
    /// New-file line numbers exposed by the change and its context.
    pub seen_lines: Vec<usize>,
}

/// Aggregate outcome for a complete patch.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ApplyPatchReport {
    /// Number of added files.
    pub added: usize,
    /// Number of updated files, including moves.
    pub updated: usize,
    /// Number of deleted files.
    pub deleted: usize,
    /// Number of moved files.
    pub moved: usize,
    /// File outcomes in patch order.
    pub files: Vec<ApplyPatchFileReport>,
}

/// Protocol-independent complete patch executor.
#[derive(Debug, Clone, Copy, Default)]
pub struct ApplyPatchExecutor;

impl ApplyPatchExecutor {
    /// Parse and transactionally apply a complete patch. Path authorization is handled by the
    /// caller's execution permission policy; this layer only resolves paths and performs I/O.
    pub fn apply(
        self,
        workspace_root: &Path,
        patch_text: &str,
    ) -> Result<ApplyPatchReport, ApplyPatchError> {
        let root = workspace_root
            .canonicalize()
            .map_err(|error| ApplyPatchError::new(format!("PathDiff 工作区不可访问：{error}")))?;
        let hunks = parse_patch(patch_text)
            .map_err(|error| ApplyPatchError::new(format!("PathDiff 解析失败：{error}")))?;
        let snapshot = read_snapshot(&root, &hunks)?;
        let rendered_diffs = rendered_diffs(&hunks, &snapshot);
        let mut plan = compute_plan(&hunks, &snapshot).map_err(|error| {
            ApplyPatchError::new(format_patch_apply_diagnostic(&error, &snapshot))
        })?;
        inherit_plan_text_formats(&mut plan, &snapshot);
        let report = report_for_plan(&plan, &snapshot, &rendered_diffs)?;
        materialize(&root, &plan, &snapshot).map_err(|error| {
            // 事务状态的唯一权威来源是**实际执行回滚的那段代码**（`rollback`）：
            // 调用方无从得知回滚是否成功，此处若断言「已全部安全回滚」会在
            // 回滚失败时与紧随其后的失败明细自相矛盾（违反 INV-3）。
            ApplyPatchError::new(format!(
                "[builtin.edit 事务异常: TRANSACTION_ABORTED]\n\
                 【失败原因】：{error}"
            ))
        })?;
        Ok(report)
    }
}

fn read_snapshot(
    root: &Path,
    hunks: &[PatchHunk],
) -> Result<HashMap<String, String>, ApplyPatchError> {
    let mut snapshot = HashMap::new();
    for hunk in hunks {
        if snapshot.contains_key(hunk.source_path()) {
            continue;
        }
        let path = safe_workspace_join(root, hunk.source_path())?;
        // Add File 也可能命中已存在文件：此时同样需要旧内容，compute_plan 才能把它
        // 规划为整文件重写（Update），从而复用备份/回滚链路并继承原文件的行尾与 BOM。
        // 目标缺失是正常「新建」；不可按 UTF-8 文本读取（目录/二进制）时保持既有语义，
        // 由写入前的「目标已存在」检查给出明确失败。
        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(_) if matches!(hunk, PatchHunk::AddFile { .. }) => continue,
            Err(error) => {
                return Err(ApplyPatchError::new(format!(
                    "PathDiff 读取 {} 失败：{error}",
                    hunk.source_path()
                )));
            }
        };
        snapshot.insert(hunk.source_path().to_owned(), content);
    }
    Ok(snapshot)
}

fn inherit_plan_text_formats(plan: &mut PatchPlan, snapshot: &HashMap<String, String>) {
    for operation in &mut plan.operations {
        let PatchFileChange::Update { new_content, .. } = &mut operation.change else {
            continue;
        };
        if let Some(existing) = snapshot.get(&operation.path) {
            *new_content = inherit_text_file_format(existing, new_content);
        }
    }
}

fn report_for_plan(
    plan: &PatchPlan,
    snapshot: &HashMap<String, String>,
    rendered_diffs: &HashMap<String, String>,
) -> Result<ApplyPatchReport, ApplyPatchError> {
    let mut report = ApplyPatchReport {
        files: Vec::with_capacity(plan.operations.len()),
        ..ApplyPatchReport::default()
    };
    for operation in &plan.operations {
        match &operation.change {
            PatchFileChange::Add { content } => {
                report.added += 1;
                report.files.push(ApplyPatchFileReport {
                    path: operation.path.clone(),
                    source_path: None,
                    operation: "add".to_owned(),
                    diff: rendered_diffs
                        .get(&operation.path)
                        .cloned()
                        .unwrap_or_else(|| content.clone()),
                    seen_lines: (1..=text_file_line_count(content)).collect(),
                });
            }
            PatchFileChange::Delete { previous_content } => {
                report.deleted += 1;
                report.files.push(ApplyPatchFileReport {
                    path: operation.path.clone(),
                    source_path: None,
                    operation: "delete".to_owned(),
                    diff: previous_content.clone().unwrap_or_default(),
                    seen_lines: Vec::new(),
                });
            }
            PatchFileChange::Update {
                move_path,
                new_content,
            } => {
                report.updated += 1;
                let target = move_path.as_deref().unwrap_or(&operation.path);
                let moved = target != operation.path;
                if moved {
                    report.moved += 1;
                }
                let existing = snapshot.get(&operation.path).ok_or_else(|| {
                    ApplyPatchError::new(format!("PathDiff 缺少源文件快照：{}", operation.path))
                })?;
                report.files.push(ApplyPatchFileReport {
                    path: target.to_owned(),
                    source_path: moved.then(|| operation.path.clone()),
                    operation: if moved { "move" } else { "update" }.to_owned(),
                    diff: rendered_diffs
                        .get(target)
                        .cloned()
                        .unwrap_or_else(|| new_content.clone()),
                    seen_lines: text_change_visible_lines(
                        existing,
                        new_content,
                        VISIBLE_CONTEXT_RADIUS,
                    ),
                });
            }
        }
    }
    Ok(report)
}

fn rendered_diffs(
    hunks: &[PatchHunk],
    snapshot: &HashMap<String, String>,
) -> HashMap<String, String> {
    hunks
        .iter()
        .map(|hunk| {
            let diff = match hunk {
                // 覆盖已存在文件时渲染真实的前后对比，避免把重写显示成新增。
                PatchHunk::AddFile { path, contents } => match snapshot.get(path) {
                    Some(existing) => whole_file_rewrite_diff(existing, contents),
                    None => contents.clone(),
                },
                PatchHunk::DeleteFile { .. } => String::new(),
                PatchHunk::UpdateFile { chunks, .. } => chunks
                    .iter()
                    .flat_map(|chunk| {
                        std::iter::once("@@".to_owned())
                            .chain(chunk.old_lines.iter().map(|line| format!("-{line}")))
                            .chain(chunk.new_lines.iter().map(|line| format!("+{line}")))
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
            };
            (hunk.path().to_owned(), diff)
        })
        .collect()
}

/// 整文件重写的可读 diff：旧行全部标 `-`，新行全部标 `+`。
fn whole_file_rewrite_diff(existing: &str, contents: &str) -> String {
    std::iter::once("@@".to_owned())
        .chain(existing.lines().map(|line| format!("-{line}")))
        .chain(contents.lines().map(|line| format!("+{line}")))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Normalize a workspace path for display and planning.
pub fn normalize_relative_path(path: &str) -> Result<String, ApplyPatchError> {
    if path.is_empty() {
        return Err(ApplyPatchError::new("PathDiff 路径不能为空"));
    }
    Ok(path.replace('\\', "/"))
}

/// Read an optional UTF-8 file using the supplied workspace as the base for relative paths.
pub fn read_workspace_file(
    root: &Path,
    relative_path: &str,
) -> Result<Option<String>, ApplyPatchError> {
    let root = canonical_root(root)?;
    let target = safe_workspace_join(&root, relative_path)?;
    match fs::read_to_string(&target) {
        Ok(content) => Ok(Some(content)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(io_error("读取工作区文件", &target, &error)),
    }
}

/// Check whether a target path already exists.
pub fn workspace_target_exists(root: &Path, relative_path: &str) -> Result<bool, ApplyPatchError> {
    let root = canonical_root(root)?;
    let target = safe_workspace_join(&root, relative_path)?;
    match fs::symlink_metadata(target) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(io_error("检查工作区路径", root.as_ref(), &error)),
    }
}

/// Apply a precomputed plan through the same strict transactional materializer as [`ApplyPatchExecutor`].
pub fn transactionally_materialize(
    root: &Path,
    plan: &PatchPlan,
    snapshot: &HashMap<String, String>,
) -> Result<(), ApplyPatchError> {
    materialize(&canonical_root(root)?, plan, snapshot)
}

fn canonical_root(root: &Path) -> Result<PathBuf, ApplyPatchError> {
    root.canonicalize()
        .map_err(|error| ApplyPatchError::new(format!("PathDiff 工作区不可访问：{error}")))
}

fn io_error(operation: &str, path: &Path, error: &std::io::Error) -> ApplyPatchError {
    ApplyPatchError::new(format!(
        "PathDiff {operation} {} 失败：{error}",
        path.display()
    ))
}
