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
    // 备份是恢复手段不是历史账本：写成功之后只留最近几份。
    // 清理失败不能影响本次写入的结论，这里不把错误往上报。
    prune_backups(path);
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

/// 每份配置文件最多保留的备份份数。
const BACKUP_RETENTION: usize = 3;

/// 保留目标文件最近 `BACKUP_RETENTION` 份备份，删掉更早的。
///
/// 备份名形如 `{文件名}.backup-{毫秒时间戳}[-{序号}]`，按名字里的时间戳排序即可
/// 还原新旧，不依赖 mtime（mtime 会被外部工具改写）。格式认不出的条目一律不删，
/// 交给人处理，避免误删别的东西。
fn prune_backups(path: &Path) {
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return;
    };
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let prefix = format!("{file_name}.backup-");
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    let mut backups: Vec<(u128, u32, PathBuf)> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name();
            let rest = name.to_str()?.strip_prefix(prefix.as_str())?;
            let (stamp, suffix) = match rest.split_once('-') {
                Some((stamp, suffix)) => (stamp, suffix.parse::<u32>().ok()?),
                None => (rest, 0),
            };
            Some((stamp.parse::<u128>().ok()?, suffix, entry.path()))
        })
        .collect();
    if backups.len() <= BACKUP_RETENTION {
        return;
    }
    backups.sort_unstable_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
    let stale = backups.len() - BACKUP_RETENTION;
    for (_, _, path) in &backups[..stale] {
        let _ = fs::remove_file(path);
    }
}
