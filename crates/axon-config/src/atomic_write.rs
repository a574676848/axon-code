//! 配置文件的原子替换写入。
//!
//! 复用仓库既有的成熟范式（`tempfile::NamedTempFile::new_in` 加 `write_all` 加 `sync_all`
//! 加 `persist`）：临时文件与目标文件始终位于同一目录，因此重命名不跨卷；
//! 内容先刷盘再做原子替换，消除「直接覆写时进程中断留下半截配置」的崩溃窗口。

use std::{fs, io::Write, path::Path};

use axon_domain::{AxonError, AxonResult};
use tempfile::NamedTempFile;

/// 把 `bytes` 原子写入 `path`。
///
/// 步骤固定为：创建父目录、写同目录临时文件、刷盘、沿用既有权限位、原子替换。
/// 任一前置步骤失败都在替换前中止，目标文件保持原内容不被破坏；
/// 替换失败时 `NamedTempFile` 的析构会清理临时文件，不残留半截内容。
pub fn replace_file_atomically(path: &Path, bytes: &[u8]) -> AxonResult<()> {
    let parent = parent_directory(path);
    fs::create_dir_all(parent)
        .map_err(|error| AxonError::runtime_persistence(format!("无法创建配置目录：{error}")))?;

    let mut temporary = NamedTempFile::new_in(parent).map_err(|error| {
        AxonError::runtime_persistence(format!("无法创建配置临时文件：{error}"))
    })?;
    temporary
        .write_all(bytes)
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|error| {
            AxonError::runtime_persistence(format!("无法写入配置临时文件：{error}"))
        })?;

    make_permissions_match(path, temporary.path());

    temporary.persist(path).map_err(|error| {
        AxonError::runtime_persistence(format!("无法原子替换配置文件：{}", error.error))
    })?;
    Ok(())
}

/// 取目标文件的父目录；路径没有父目录时回落到当前目录。
fn parent_directory(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

/// 目标文件已存在时沿用其权限位，避免临时文件的默认权限放宽既有约束。
fn make_permissions_match(existing: &Path, temporary: &Path) {
    if let Ok(metadata) = fs::metadata(existing) {
        let _ = fs::set_permissions(temporary, metadata.permissions());
    }
}
