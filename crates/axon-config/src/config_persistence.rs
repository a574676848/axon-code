//! 用户配置的备份与直接写入。

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use axon_domain::{AxonError, AxonResult};

use crate::{AxonConfig, ConfigLoader, to_toml_string};

impl ConfigLoader {
    /// 保存用户配置。
    ///
    /// 内容未变时直接返回；否则先创建时间戳备份，再直接写入原文件并刷盘。
    /// 保留原文件身份与权限，兼容 Windows 编辑器允许写入但禁止替换的句柄。
    /// 此操作不保证原子性，中断后的恢复应使用写入前备份。
    pub fn save_to_path(path: &Path, config: &AxonConfig) -> AxonResult<()> {
        let text = to_toml_string(config)?;
        if fs::read(path).is_ok_and(|existing| existing == text.as_bytes()) {
            return Ok(());
        }
        write_with_backup(path, text.as_bytes())?;
        Ok(())
    }
}

/// 备份原文件后直接写入 `bytes`，返回备份路径。
///
/// 这是本项目对「可能被其它进程持有时仍要写入」的既定策略（见 `save_to_path`）：
/// 先备份，再以打开并截断的方式覆写，保留原文件身份与权限。之所以不用原子替换，
/// 是因为 Windows 上另一进程若以「允许写入、禁止删除」的方式持有文件，
/// 临时文件 `persist` 会以「拒绝访问 (os error 5)」失败，而覆写不会。
/// 代价是写入过程不保证原子性：中断后的恢复依赖这里留下的备份。
pub fn write_with_backup(path: &Path, bytes: &[u8]) -> AxonResult<Option<PathBuf>> {
    let backup = backup_before_update(path)?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    if let Some(parent) = parent {
        fs::create_dir_all(parent).map_err(|error| {
            AxonError::runtime_persistence(format!("无法创建配置目录：{error}"))
        })?;
    }
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .map_err(|error| {
            AxonError::runtime_persistence(format!("无法打开配置文件进行写入：{error}"))
        })?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| AxonError::runtime_persistence(format!("无法写入配置文件：{error}")))?;
    Ok(backup)
}

fn backup_before_update(path: &Path) -> AxonResult<Option<PathBuf>> {
    if !path.exists() {
        return Ok(None);
    }
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| AxonError::runtime_persistence("配置文件名无效，无法创建备份"))?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| AxonError::runtime_persistence("系统时间异常，无法创建配置备份"))?
        .as_millis();
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    for suffix in 0_u32.. {
        let unique = if suffix == 0 {
            timestamp.to_string()
        } else {
            format!("{timestamp}-{suffix}")
        };
        let backup = parent.join(format!("{file_name}.backup-{unique}"));
        if backup.exists() {
            continue;
        }
        fs::copy(path, &backup).map_err(|error| {
            AxonError::runtime_persistence(format!("无法在更新前备份配置文件：{error}"))
        })?;
        return Ok(Some(backup));
    }
    unreachable!("u32 backup suffix space is exhausted")
}
