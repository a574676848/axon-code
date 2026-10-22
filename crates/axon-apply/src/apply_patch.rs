//! Transactional application of complete text patches inside one workspace.

use std::collections::{BTreeSet, HashMap};
use std::fs::{self, File, Permissions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use axon_patch_model::{
    PatchFileChange, PatchHunk, PatchPlan, compute_plan, inherit_text_file_format, parse_patch,
    text_change_visible_lines, text_file_line_count,
};

#[path = "apply_patch_path_validation.rs"]
mod path_validation;
use path_validation::safe_workspace_join;
#[path = "apply_patch_diagnostic.rs"]
mod diagnostic;
use diagnostic::format_patch_apply_diagnostic;

#[cfg(test)]
#[path = "apply_patch_tests.rs"]
mod tests;

const VISIBLE_CONTEXT_RADIUS: usize = 3;
const TRANSACTION_DIR_PREFIX: &str = ".axon-apply-patch-txn";
const TRANSACTION_DIR_CREATE_ATTEMPTS: usize = 64;

static TRANSACTION_SEQUENCE: AtomicU64 = AtomicU64::new(1);
static WORKSPACE_LOCKS: OnceLock<Mutex<HashMap<PathBuf, Weak<Mutex<()>>>>> = OnceLock::new();

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

fn materialize(
    root: &Path,
    plan: &PatchPlan,
    snapshot: &HashMap<String, String>,
) -> Result<(), ApplyPatchError> {
    let lock = workspace_lock(root)?;
    let _guard = lock
        .lock()
        .map_err(|_| ApplyPatchError::new("PathDiff 工作区写锁已损坏"))?;
    let mut entries = build_entries(root, plan, snapshot)?;
    verify_preconditions(&mut entries)?;
    let mut transaction = TransactionRoot::create(root)?;
    stage_contents(transaction.path(), &mut entries)?;
    commit(&mut transaction, &mut entries)
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

fn workspace_lock(root: &Path) -> Result<Arc<Mutex<()>>, ApplyPatchError> {
    let locks = WORKSPACE_LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut locks = locks
        .lock()
        .map_err(|_| ApplyPatchError::new("PathDiff 工作区锁注册表已损坏"))?;
    locks.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = locks.get(root).and_then(Weak::upgrade) {
        return Ok(lock);
    }
    let lock = Arc::new(Mutex::new(()));
    locks.insert(root.to_path_buf(), Arc::downgrade(&lock));
    Ok(lock)
}

fn build_entries(
    root: &Path,
    plan: &PatchPlan,
    snapshot: &HashMap<String, String>,
) -> Result<Vec<Entry>, ApplyPatchError> {
    let mut reserved = BTreeSet::new();
    let mut entries = Vec::with_capacity(plan.operations.len());
    for operation in &plan.operations {
        let source = safe_workspace_join(root, &operation.path)?;
        let entry = match &operation.change {
            PatchFileChange::Add { content } => Entry::addition(source, content.clone()),
            PatchFileChange::Delete { .. } => {
                Entry::deletion(source, expected(snapshot, &operation.path)?)
            }
            PatchFileChange::Update {
                move_path,
                new_content,
            } => Entry::update(
                source,
                safe_workspace_join(root, move_path.as_deref().unwrap_or(&operation.path))?,
                expected(snapshot, &operation.path)?,
                new_content.clone(),
            ),
        };
        for path in entry.reserved_paths() {
            if !reserved.insert(path.to_path_buf()) {
                return Err(ApplyPatchError::new(format!(
                    "PathDiff 同一批次重复修改文件：{}",
                    path.display()
                )));
            }
        }
        entries.push(entry);
    }
    Ok(entries)
}

fn expected(snapshot: &HashMap<String, String>, path: &str) -> Result<String, ApplyPatchError> {
    snapshot
        .get(path)
        .cloned()
        .ok_or_else(|| ApplyPatchError::new(format!("PathDiff 的源文件不存在：{path}")))
}

fn verify_preconditions(entries: &mut [Entry]) -> Result<(), ApplyPatchError> {
    for entry in entries {
        if let Some(source) = entry.source.as_deref() {
            let metadata = fs::metadata(source)
                .map_err(|error| io_error("读取源文件元数据", source, &error))?;
            if !metadata.is_file() {
                return Err(ApplyPatchError::new(format!(
                    "PathDiff 源路径不是文件：{}",
                    source.display()
                )));
            }
            entry.permissions = Some(metadata.permissions());
            let actual = fs::read_to_string(source)
                .map_err(|error| io_error("读取源文件", source, &error))?;
            if entry.expected_source.as_deref() != Some(actual.as_str()) {
                return Err(ApplyPatchError::new(format!(
                    "PathDiff 写入前检测到并发修改：{}",
                    source.display()
                )));
            }
        }
        if entry.source.as_ref() != Some(&entry.target) {
            match fs::symlink_metadata(&entry.target) {
                Ok(_) => {
                    return Err(ApplyPatchError::new(format!(
                        "PathDiff 目标已存在：{}",
                        entry.target.display()
                    )));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(io_error("检查目标路径", &entry.target, &error)),
            }
        }
    }
    Ok(())
}

fn stage_contents(transaction: &Path, entries: &mut [Entry]) -> Result<(), ApplyPatchError> {
    let staged = transaction.join("staged");
    fs::create_dir(&staged).map_err(|error| io_error("创建预写目录", &staged, &error))?;
    for (index, entry) in entries.iter_mut().enumerate() {
        let Some(content) = entry.content.as_deref() else {
            continue;
        };
        let path = staged.join(index.to_string());
        let mut file =
            File::create(&path).map_err(|error| io_error("创建预写文件", &path, &error))?;
        file.write_all(content.as_bytes())
            .and_then(|()| file.sync_all())
            .map_err(|error| io_error("持久化预写文件", &path, &error))?;
        if let Some(permissions) = entry.permissions.clone() {
            fs::set_permissions(&path, permissions)
                .map_err(|error| io_error("复制文件权限", &path, &error))?;
        }
        entry.staged_path = Some(path);
    }
    Ok(())
}

fn commit(transaction: &mut TransactionRoot, entries: &mut [Entry]) -> Result<(), ApplyPatchError> {
    let mut created_directories = Vec::new();
    if let Err(error) = create_target_directories(entries, &mut created_directories) {
        cleanup_directories(&created_directories);
        return Err(error);
    }
    let backups = transaction.path().join("backup");
    if let Err(error) = fs::create_dir(&backups) {
        cleanup_directories(&created_directories);
        return Err(io_error("创建备份目录", &backups, &error));
    }
    for (index, entry) in entries.iter_mut().enumerate() {
        if let Err(error) = backup_source(&backups, index, entry) {
            return rollback(transaction, entries, &created_directories, error);
        }
    }
    for index in 0..entries.len() {
        let promotion_error = {
            let entry = &entries[index];
            entry.staged_path.as_deref().and_then(|staged| {
                fs::hard_link(staged, &entry.target)
                    .err()
                    .map(|error| io_error("提交预写文件", &entry.target, &error))
            })
        };
        if let Some(error) = promotion_error {
            return rollback(transaction, entries, &created_directories, error);
        }
        entries[index].promoted = entries[index].staged_path.is_some();
    }
    Ok(())
}

fn create_target_directories(
    entries: &[Entry],
    created: &mut Vec<PathBuf>,
) -> Result<(), ApplyPatchError> {
    for entry in entries.iter().filter(|entry| entry.content.is_some()) {
        let Some(parent) = entry.target.parent() else {
            continue;
        };
        let mut missing = Vec::new();
        let mut current = parent;
        loop {
            match fs::metadata(current) {
                Ok(metadata) if metadata.is_dir() => break,
                Ok(_) => {
                    return Err(ApplyPatchError::new(format!(
                        "PathDiff 目标父路径不是目录：{}",
                        current.display()
                    )));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    missing.push(current.to_path_buf());
                    current = current
                        .parent()
                        .ok_or_else(|| ApplyPatchError::new("PathDiff 目标路径缺少父目录"))?;
                }
                Err(error) => return Err(io_error("检查目标目录", current, &error)),
            }
        }
        for directory in missing.into_iter().rev() {
            fs::create_dir(&directory)
                .map_err(|error| io_error("创建目标目录", &directory, &error))?;
            created.push(directory);
        }
    }
    Ok(())
}

fn backup_source(backups: &Path, index: usize, entry: &mut Entry) -> Result<(), ApplyPatchError> {
    let Some(source) = entry.source.as_deref() else {
        return Ok(());
    };
    let backup = backups.join(index.to_string());
    fs::rename(source, &backup).map_err(|error| io_error("备份源文件", source, &error))?;
    entry.backup_path = Some(backup.clone());
    let actual =
        fs::read_to_string(&backup).map_err(|error| io_error("复核备份文件", &backup, &error))?;
    if entry.expected_source.as_deref() != Some(actual.as_str()) {
        return Err(ApplyPatchError::new(format!(
            "PathDiff 源文件在事务提交前发生变化：{}",
            source.display()
        )));
    }
    Ok(())
}

fn rollback(
    transaction: &mut TransactionRoot,
    entries: &mut [Entry],
    created: &[PathBuf],
    cause: ApplyPatchError,
) -> Result<(), ApplyPatchError> {
    let mut failures = Vec::new();
    for entry in entries.iter_mut().rev().filter(|entry| entry.promoted) {
        if let Err(error) = fs::remove_file(&entry.target)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            failures.push(format!("删除 {} 失败：{error}", entry.target.display()));
        }
        entry.promoted = false;
    }
    for entry in entries.iter_mut().rev() {
        let (Some(source), Some(backup)) = (entry.source.as_deref(), entry.backup_path.as_deref())
        else {
            continue;
        };
        if let Err(error) = fs::rename(backup, source) {
            failures.push(format!("恢复 {} 失败：{error}", source.display()));
        }
    }
    cleanup_directories(created);
    if failures.is_empty() {
        Err(ApplyPatchError::new(format!(
            "{cause}\n【事务状态】：本次修改已全部安全回滚，工作区未产生脏状态。"
        )))
    } else {
        // 备份只存在于事务目录内，而 `Drop` 会无条件清空该目录：不保留它，
        // 「可恢复的写失败」就升级为**永久数据丢失**。
        transaction.preserve();
        Err(ApplyPatchError::new(format!(
            "{cause}\n\
             【事务状态】：回滚**未完成**，工作区可能残留未恢复的文件，请人工核对后再继续。\n\
             【未恢复备份】：{}\n\
             【回滚失败明细】：{}",
            transaction.path().display(),
            failures.join("；")
        )))
    }
}

fn cleanup_directories(directories: &[PathBuf]) {
    for directory in directories.iter().rev() {
        let _ = fs::remove_dir(directory);
    }
}

fn io_error(operation: &str, path: &Path, error: &std::io::Error) -> ApplyPatchError {
    ApplyPatchError::new(format!(
        "PathDiff {operation} {} 失败：{error}",
        path.display()
    ))
}

struct Entry {
    source: Option<PathBuf>,
    target: PathBuf,
    expected_source: Option<String>,
    content: Option<String>,
    permissions: Option<Permissions>,
    staged_path: Option<PathBuf>,
    backup_path: Option<PathBuf>,
    promoted: bool,
}

impl Entry {
    fn addition(target: PathBuf, content: String) -> Self {
        Self::new(None, target, None, Some(content))
    }
    fn deletion(source: PathBuf, expected: String) -> Self {
        Self::new(Some(source.clone()), source, Some(expected), None)
    }
    fn update(source: PathBuf, target: PathBuf, expected: String, content: String) -> Self {
        Self::new(Some(source), target, Some(expected), Some(content))
    }
    fn new(
        source: Option<PathBuf>,
        target: PathBuf,
        expected_source: Option<String>,
        content: Option<String>,
    ) -> Self {
        Self {
            source,
            target,
            expected_source,
            content,
            permissions: None,
            staged_path: None,
            backup_path: None,
            promoted: false,
        }
    }
    fn reserved_paths(&self) -> impl Iterator<Item = &Path> {
        std::iter::once(self.target.as_path()).chain(
            self.source
                .iter()
                .map(PathBuf::as_path)
                .filter(|path| *path != self.target),
        )
    }
}

struct TransactionRoot {
    path: PathBuf,
    /// 回滚失败时置位：`Drop` 不再删除事务目录。
    ///
    /// 备份只存在于该目录内，是目标文件可能**唯一的副本**；无条件删除会把
    /// 「可恢复的写失败」升级为永久数据丢失。
    preserve: bool,
}

impl TransactionRoot {
    fn create(root: &Path) -> Result<Self, ApplyPatchError> {
        for _ in 0..TRANSACTION_DIR_CREATE_ATTEMPTS {
            let sequence = TRANSACTION_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = root.join(format!(
                "{TRANSACTION_DIR_PREFIX}-{}-{sequence}",
                std::process::id()
            ));
            match fs::create_dir(&path) {
                Ok(()) => {
                    return Ok(Self {
                        path,
                        preserve: false,
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(io_error("创建事务目录", &path, &error)),
            }
        }
        Err(ApplyPatchError::new("PathDiff 无法分配唯一事务目录"))
    }
    fn path(&self) -> &Path {
        &self.path
    }
    /// 标记为保留：`Drop` 不再删除事务目录，供人工恢复未还原的备份。
    fn preserve(&mut self) {
        self.preserve = true;
    }
}

impl Drop for TransactionRoot {
    fn drop(&mut self) {
        if self.preserve {
            return;
        }
        let _ = fs::remove_dir_all(&self.path);
    }
}
