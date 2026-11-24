//! builtin.edit 的事务落盘：备份、预写、提升、回滚与跨卷回落。
//!
//! 事务目录固定建在工作区根，因此备份与提升默认只用同卷原语：`rename`（把源文件移进
//! 备份目录）与 `hard_link`（把预写文件提升为目标）。两者在目标或源文件落在别的卷上时
//! 都会返回 `CrossesDevices`（Windows `os error 17`，Unix `EXDEV`），此时逐条降级：
//!
//! - 备份：改在源文件同卷的就近隐藏文件上完成 `rename`。语义仍是「源文件在备份阶段离开
//!   原位」，回滚同样用同卷 `rename` 还原，因此不引入「源文件与备份同时存在」的中间态。
//! - 提升：先把预写内容写成目标同卷的临时文件并 `fsync`，再 `rename` 覆盖目标，保持
//!   原子可见，不会出现半写文件。
//!
//! 同卷路径上的行为、顺序与失败语义完全不变；跨卷分支只替换落盘原语，不改变事务顺序
//! （建目录 → 备份 → 提升 → 失败回滚）。跨卷备份若因回滚失败被保留，会与事务目录一并
//! 出现在错误回执里，供人工恢复。

use std::collections::{BTreeSet, HashMap};
use std::fs::{self, File, OpenOptions, Permissions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use axon_patch_model::{PatchFileChange, PatchPlan};

use super::path_validation::safe_workspace_join;
use super::{ApplyPatchError, io_error};

const TRANSACTION_DIR_PREFIX: &str = ".axon-apply-patch-txn";
const TRANSACTION_DIR_CREATE_ATTEMPTS: usize = 64;
/// 跨卷备份文件名中缀：备份与源文件同目录、同卷。
const CROSS_VOLUME_BACKUP_INFIX: &str = ".axon-apply-backup-";
/// 跨卷提升临时文件名中缀：临时文件与目标同目录、同卷。
const CROSS_VOLUME_PROMOTE_INFIX: &str = ".axon-apply-promote-";

pub(super) static TRANSACTION_SEQUENCE: AtomicU64 = AtomicU64::new(1);
static WORKSPACE_LOCKS: OnceLock<Mutex<HashMap<PathBuf, Weak<Mutex<()>>>>> = OnceLock::new();

pub(super) fn materialize(
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

pub(super) fn build_entries(
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

pub(super) fn verify_preconditions(entries: &mut [Entry]) -> Result<(), ApplyPatchError> {
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

pub(super) fn stage_contents(
    transaction: &Path,
    entries: &mut [Entry],
) -> Result<(), ApplyPatchError> {
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

pub(super) fn commit(
    transaction: &mut TransactionRoot,
    entries: &mut [Entry],
) -> Result<(), ApplyPatchError> {
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
        if let Some(error) = promote_entry(&mut entries[index]) {
            return rollback(transaction, entries, &created_directories, error);
        }
        entries[index].promoted = entries[index].staged_path.is_some();
    }
    remove_cross_volume_backups(transaction.path(), entries);
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
    let backup = match fs::rename(source, &backup) {
        Ok(()) => backup,
        Err(error) if error.kind() == std::io::ErrorKind::CrossesDevices => {
            // 源文件与工作区根不同卷：`rename` 不能跨卷移动，改在源文件同卷的就近位置
            // 备份，保持「备份阶段源文件离开原位」的既有顺序与回滚语义。
            let local = cross_volume_backup_path(source)?;
            fs::rename(source, &local).map_err(|error| io_error("备份源文件", source, &error))?;
            local
        }
        Err(error) => return Err(io_error("备份源文件", source, &error)),
    };
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

/// 为跨卷源文件分配同卷备份名：与源文件同目录、隐藏、带进程号与序号，且必须尚未被占用。
fn cross_volume_backup_path(source: &Path) -> Result<PathBuf, ApplyPatchError> {
    let parent = source.parent().ok_or_else(|| {
        ApplyPatchError::new(format!("PathDiff 源文件缺少父目录：{}", source.display()))
    })?;
    let name = source
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("source");
    for _ in 0..TRANSACTION_DIR_CREATE_ATTEMPTS {
        let sequence = TRANSACTION_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(
            ".{name}{CROSS_VOLUME_BACKUP_INFIX}{}-{sequence}",
            std::process::id()
        ));
        match fs::symlink_metadata(&candidate) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(candidate),
            Ok(_) => {}
            Err(error) => return Err(io_error("检查跨卷备份路径", &candidate, &error)),
        }
    }
    Err(ApplyPatchError::new(format!(
        "PathDiff 无法为跨卷文件分配唯一备份名：{}",
        source.display()
    )))
}

fn promote_entry(entry: &mut Entry) -> Option<ApplyPatchError> {
    let staged = entry.staged_path.clone()?;
    match fs::hard_link(&staged, &entry.target) {
        Ok(()) => None,
        Err(error) if error.kind() == std::io::ErrorKind::CrossesDevices => {
            // 目标与工作区根不同卷：硬链不能跨卷，改为在目标同卷写临时文件再原子改名。
            promote_into_target_volume(&staged, &entry.target, entry.permissions.as_ref())
                .err()
                .map(|error| io_error("提交预写文件", &entry.target, &error))
        }
        Err(error) => Some(io_error("提交预写文件", &entry.target, &error)),
    }
}

/// 跨卷提升：把预写内容写到目标同卷的临时文件，`fsync` 后用 `rename` 覆盖目标。
///
/// `rename` 只在临时文件与目标之间发生，二者同目录同卷，因此仍然是原子替换；任何一步
/// 失败都会删掉临时文件，不留下半写内容。
fn promote_into_target_volume(
    staged: &Path,
    target: &Path,
    permissions: Option<&Permissions>,
) -> std::io::Result<()> {
    let parent = target.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "目标路径缺少父目录")
    })?;
    let name = target
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("target");
    let mut last_occupied = None;
    for _ in 0..TRANSACTION_DIR_CREATE_ATTEMPTS {
        let sequence = TRANSACTION_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temp = parent.join(format!(
            ".{name}{CROSS_VOLUME_PROMOTE_INFIX}{}-{sequence}",
            std::process::id()
        ));
        match write_promote_temp(staged, &temp, permissions) {
            Ok(()) => {
                // 与同卷 `hard_link` 保持一致的失败语义：目标若在预检后被并发创建，
                // 跨卷分支必须放弃提交，而不是用改名把对方的文件覆盖掉。
                if fs::symlink_metadata(target).is_ok() {
                    let _ = fs::remove_file(&temp);
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::AlreadyExists,
                        "目标文件已存在，跨卷提交已放弃",
                    ));
                }
                if let Err(error) = fs::rename(&temp, target) {
                    let _ = fs::remove_file(&temp);
                    return Err(error);
                }
                return Ok(());
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                last_occupied = Some(error);
            }
            Err(error) => return Err(error),
        }
    }
    Err(last_occupied.unwrap_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "跨卷提交无法分配唯一的临时文件",
        )
    }))
}

fn write_promote_temp(
    staged: &Path,
    temp: &Path,
    permissions: Option<&Permissions>,
) -> std::io::Result<()> {
    let content = fs::read(staged)?;
    let mut file = OpenOptions::new().write(true).create_new(true).open(temp)?;
    let written = file.write_all(&content).and_then(|()| file.sync_all());
    if let Err(error) = written {
        drop(file);
        let _ = fs::remove_file(temp);
        return Err(error);
    }
    if let Some(permissions) = permissions {
        fs::set_permissions(temp, permissions.clone())?;
    }
    Ok(())
}

/// 提交成功后清掉跨卷备份：它们散落在源文件同目录，不属于事务目录，`Drop` 不会处理。
///
/// 删除失败只会在源目录留下一个隐藏备份，已提交内容不受影响，因此不升级为工具失败。
fn remove_cross_volume_backups(transaction: &Path, entries: &[Entry]) {
    for backup in entries
        .iter()
        .filter_map(|entry| entry.backup_path.as_deref())
    {
        if !backup.starts_with(transaction) {
            let _ = fs::remove_file(backup);
        }
    }
}

fn cross_volume_backups(transaction: &Path, entries: &[Entry]) -> Vec<String> {
    entries
        .iter()
        .filter_map(|entry| entry.backup_path.as_deref())
        .filter(|backup| !backup.starts_with(transaction) && backup.exists())
        .map(|backup| backup.display().to_string())
        .collect()
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
        let mut message = format!(
            "{cause}\n\
              【事务状态】：回滚**未完成**，工作区可能残留未恢复的文件，请人工核对后再继续。\n\
              【未恢复备份】：{}",
            transaction.path().display(),
        );
        let external = cross_volume_backups(transaction.path(), entries);
        if !external.is_empty() {
            message.push_str(&format!("\n【未恢复跨卷备份】：{}", external.join("；")));
        }
        message.push_str(&format!("\n【回滚失败明细】：{}", failures.join("；")));
        Err(ApplyPatchError::new(message))
    }
}

fn cleanup_directories(directories: &[PathBuf]) {
    for directory in directories.iter().rev() {
        let _ = fs::remove_dir(directory);
    }
}

pub(super) struct Entry {
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

pub(super) struct TransactionRoot {
    path: PathBuf,
    /// 回滚失败时置位：`Drop` 不再删除事务目录。
    ///
    /// 备份只存在于该目录内，是目标文件可能**唯一的副本**；无条件删除会把
    /// 「可恢复的写失败」升级为永久数据丢失。
    preserve: bool,
}

impl TransactionRoot {
    pub(super) fn create(root: &Path) -> Result<Self, ApplyPatchError> {
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
    pub(super) fn path(&self) -> &Path {
        &self.path
    }
    /// 标记为保留：`Drop` 不再删除事务目录，供人工恢复未还原的备份。
    pub(super) fn preserve(&mut self) {
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
