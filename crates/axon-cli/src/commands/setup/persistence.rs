use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const SETUP_LOCK_FILE_NAME: &str = ".setup.lock";
const SETUP_JOURNAL_FILE_NAME: &str = ".setup-journal.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum JournalPhase {
    Prepared,
    ConfigCommitted,
    EnvCommitted,
    Completed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct JournalFile {
    pub target: PathBuf,
    pub existed: bool,
    pub backup: Option<PathBuf>,
    pub temp: Option<PathBuf>,
    pub candidate_sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SetupJournal {
    pub version: u8,
    pub transaction_id: String,
    pub config: JournalFile,
    pub env: JournalFile,
    pub phase: JournalPhase,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct FileStateReport {
    pub path: String,
    pub state: String,
    #[serde(rename = "backupPath", skip_serializing_if = "Option::is_none")]
    pub backup_path: Option<String>,
}

pub(crate) struct SetupTransaction {
    lock_file: File,
    journal_path: PathBuf,
    pub journal: SetupJournal,
    pub config_snapshot_sha256: Option<String>,
    pub env_snapshot_sha256: Option<String>,
}

impl SetupTransaction {
    pub(crate) fn begin(config_path: &Path, env_path: &Path) -> Result<Self, String> {
        let parent = config_path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent).map_err(|e| format!("无法创建配置目录: {e}"))?;

        let lock_path = parent.join(SETUP_LOCK_FILE_NAME);
        let lock_file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|e| format!("无法打开 Setup 锁文件: {e}"))?;

        lock_file
            .try_lock()
            .map_err(|_| "无法获取 Setup 独占锁，另一个 Setup 进程正在运行".to_owned())?;

        let journal_path = parent.join(SETUP_JOURNAL_FILE_NAME);
        recover_crash_if_any(&journal_path)?;

        let tx_id = format!(
            "{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
        );

        let cfg_existed = config_path.exists();
        let cfg_sha = read_snapshot_sha(config_path, cfg_existed, "配置文件")?;

        let env_existed = env_path.exists();
        let env_sha = read_snapshot_sha(env_path, env_existed, ".env 文件")?;

        let journal = SetupJournal {
            version: 1,
            transaction_id: tx_id,
            config: JournalFile {
                target: config_path.to_path_buf(),
                existed: cfg_existed,
                backup: None,
                temp: None,
                candidate_sha256: None,
            },
            env: JournalFile {
                target: env_path.to_path_buf(),
                existed: env_existed,
                backup: None,
                temp: None,
                candidate_sha256: None,
            },
            phase: JournalPhase::Prepared,
        };

        Ok(Self {
            lock_file,
            journal_path,
            journal,
            config_snapshot_sha256: cfg_sha,
            env_snapshot_sha256: env_sha,
        })
    }

    pub(crate) fn prepare(
        &mut self,
        cfg_candidate: Option<&[u8]>,
        env_candidate: Option<&[u8]>,
    ) -> Result<bool, String> {
        let parent = self
            .journal
            .config
            .target
            .parent()
            .unwrap_or_else(|| Path::new("."));

        verify_snapshot(
            &self.journal.config,
            self.config_snapshot_sha256.as_deref(),
            "配置文件",
        )?;
        verify_snapshot(
            &self.journal.env,
            self.env_snapshot_sha256.as_deref(),
            ".env 文件",
        )?;

        let mut has_work = false;

        if let Some(cfg_bytes) = cfg_candidate {
            has_work = true;
            let temp_name = format!(".setup-{}-config.tmp", self.journal.transaction_id);
            let temp_path = parent.join(temp_name);
            write_and_sync_temp(&temp_path, cfg_bytes)?;
            self.journal.config.temp = Some(temp_path);
            self.journal.config.candidate_sha256 = Some(sha256_hex(cfg_bytes));

            if self.journal.config.existed {
                let backup_path = create_backup_file(&self.journal.config.target)?;
                self.journal.config.backup = Some(backup_path);
            }
        }

        if let Some(env_bytes) = env_candidate {
            has_work = true;
            let temp_name = format!(".setup-{}-env.tmp", self.journal.transaction_id);
            let temp_path = parent.join(temp_name);
            write_and_sync_temp(&temp_path, env_bytes)?;
            self.journal.env.temp = Some(temp_path);
            self.journal.env.candidate_sha256 = Some(sha256_hex(env_bytes));

            if self.journal.env.existed {
                let backup_path = create_backup_file(&self.journal.env.target)?;
                self.journal.env.backup = Some(backup_path);
            }
        }

        if !has_work {
            return Ok(false);
        }

        self.journal.phase = JournalPhase::Prepared;
        save_journal(&self.journal_path, &self.journal)?;
        Ok(true)
    }

    pub(crate) fn commit(&mut self) -> Result<Vec<FileStateReport>, String> {
        let mut reports = Vec::new();

        if let Some(ref temp_path) = self.journal.config.temp {
            replace_target(temp_path, &self.journal.config.target)
                .map_err(|e| format!("提交配置文件失败: {e}"))?;
            self.journal.phase = JournalPhase::ConfigCommitted;
            save_journal(&self.journal_path, &self.journal)?;
            reports.push(FileStateReport {
                path: self.journal.config.target.display().to_string(),
                state: "committed".to_owned(),
                backup_path: self
                    .journal
                    .config
                    .backup
                    .as_ref()
                    .map(|p| p.display().to_string()),
            });
        }

        if let Some(ref temp_path) = self.journal.env.temp {
            replace_target(temp_path, &self.journal.env.target)
                .map_err(|e| format!("提交 env 文件失败: {e}"))?;
            self.journal.phase = JournalPhase::EnvCommitted;
            save_journal(&self.journal_path, &self.journal)?;
            reports.push(FileStateReport {
                path: self.journal.env.target.display().to_string(),
                state: "committed".to_owned(),
                backup_path: self
                    .journal
                    .env
                    .backup
                    .as_ref()
                    .map(|p| p.display().to_string()),
            });
        }

        self.journal.phase = JournalPhase::Completed;
        save_journal(&self.journal_path, &self.journal)?;
        self.cleanup();
        Ok(reports)
    }

    pub(crate) fn rollback(&self) -> Vec<FileStateReport> {
        let mut reports = Vec::new();

        let cfg_rep = rollback_single_file(&self.journal.config);
        reports.push(cfg_rep);

        let env_rep = rollback_single_file(&self.journal.env);
        reports.push(env_rep);

        if reports
            .iter()
            .all(|report| !report.state.starts_with("rollback_failed"))
        {
            self.cleanup();
        }
        reports
    }

    pub(crate) fn abort_prepare(&self) {
        self.remove_temp_files();
        if let Some(ref backup) = self.journal.config.backup {
            let _ = fs::remove_file(backup);
        }
        if let Some(ref backup) = self.journal.env.backup {
            let _ = fs::remove_file(backup);
        }
        let _ = fs::remove_file(&self.journal_path);
    }

    pub(crate) fn cleanup(&self) {
        self.remove_temp_files();
        let _ = fs::remove_file(&self.journal_path);
    }

    fn remove_temp_files(&self) {
        if let Some(ref temp) = self.journal.config.temp {
            let _ = fs::remove_file(temp);
        }
        if let Some(ref temp) = self.journal.env.temp {
            let _ = fs::remove_file(temp);
        }
    }
}

impl Drop for SetupTransaction {
    fn drop(&mut self) {
        let _ = self.lock_file.unlock();
    }
}

fn rollback_single_file(file: &JournalFile) -> FileStateReport {
    let path_str = file.target.display().to_string();
    if let Some(ref backup) = file.backup
        && backup.exists()
    {
        if let Err(e) = replace_target(backup, &file.target) {
            return FileStateReport {
                path: path_str,
                state: format!("rollback_failed: {e}"),
                backup_path: Some(backup.display().to_string()),
            };
        }
        return FileStateReport {
            path: path_str,
            state: "restored".to_owned(),
            backup_path: Some(backup.display().to_string()),
        };
    }

    if !file.existed && file.target.exists() {
        if let Err(e) = fs::remove_file(&file.target) {
            return FileStateReport {
                path: path_str,
                state: format!("rollback_failed: {e}"),
                backup_path: None,
            };
        }
        return FileStateReport {
            path: path_str,
            state: "removed".to_owned(),
            backup_path: None,
        };
    }

    FileStateReport {
        path: path_str,
        state: "unchanged".to_owned(),
        backup_path: None,
    }
}

fn recover_crash_if_any(journal_path: &Path) -> Result<(), String> {
    if !journal_path.exists() {
        return Ok(());
    }
    let text =
        fs::read_to_string(journal_path).map_err(|e| format!("无法读取遗留 Setup journal: {e}"))?;
    let journal: SetupJournal = serde_json::from_str(&text)
        .map_err(|e| format!("遗留 Setup journal 无效，已保留以便人工恢复: {e}"))?;

    if journal.phase == JournalPhase::Completed {
        remove_journal_temp_files(&journal);
        fs::remove_file(journal_path).map_err(|e| format!("无法清理已完成 Setup journal: {e}"))?;
        return Ok(());
    }

    let reports = [
        rollback_single_file(&journal.config),
        rollback_single_file(&journal.env),
    ];
    if reports
        .iter()
        .any(|report| report.state.starts_with("rollback_failed"))
    {
        return Err("遗留 Setup journal 回滚失败，已保留恢复文件".to_owned());
    }
    remove_journal_temp_files(&journal);
    fs::remove_file(journal_path).map_err(|e| format!("无法清理恢复后的 Setup journal: {e}"))?;
    Ok(())
}

fn replace_target(src: &Path, dst: &Path) -> Result<(), String> {
    if !src.exists() {
        return Err(format!("源文件不存在: {}", src.display()));
    }
    if dst.exists() {
        fs::remove_file(dst)
            .map_err(|e| format!("删除现有目标文件失败 ({}): {e}", dst.display()))?;
    }
    fs::rename(src, dst).map_err(|e| {
        format!(
            "重命名文件失败 ({} -> {}): {e}",
            src.display(),
            dst.display()
        )
    })
}

fn write_and_sync_temp(path: &Path, content: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| format!("排他创建临时文件失败 ({}): {e}", path.display()))?;
    file.write_all(content)
        .map_err(|e| format!("写入临时文件失败: {e}"))?;
    file.sync_all()
        .map_err(|e| format!("sync_all 临时文件失败: {e}"))?;
    Ok(())
}

fn create_backup_file(path: &Path) -> Result<PathBuf, String> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "文件名无效".to_owned())?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let parent = path.parent().unwrap_or_else(|| Path::new("."));

    for counter in 0_u32.. {
        let unique = if counter == 0 {
            timestamp.to_string()
        } else {
            format!("{timestamp}-{counter}")
        };
        let backup_path = parent.join(format!("{file_name}.backup-{unique}"));
        let source =
            fs::read(path).map_err(|e| format!("读取待备份文件失败 ({}): {e}", path.display()))?;
        let mut backup = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&backup_path)
        {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(format!("创建备份文件失败 ({}): {e}", backup_path.display()));
            }
        };
        backup
            .write_all(&source)
            .map_err(|e| format!("写入备份文件失败 ({}): {e}", backup_path.display()))?;
        backup
            .sync_all()
            .map_err(|e| format!("同步备份文件失败 ({}): {e}", backup_path.display()))?;
        return Ok(backup_path);
    }
    Err("备份计数器溢出".to_owned())
}

fn save_journal(path: &Path, journal: &SetupJournal) -> Result<(), String> {
    let text = serde_json::to_string_pretty(journal).map_err(|e| e.to_string())?;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let tmp_path = parent.join(format!(".setup-journal-{}.tmp", journal.transaction_id));
    write_and_sync_temp(&tmp_path, text.as_bytes())?;
    replace_target(&tmp_path, path)
}

fn read_snapshot_sha(path: &Path, existed: bool, label: &str) -> Result<Option<String>, String> {
    if !existed {
        return Ok(None);
    }
    let bytes = fs::read(path).map_err(|e| format!("读取{label}快照失败: {e}"))?;
    Ok(Some(sha256_hex(&bytes)))
}

fn verify_snapshot(
    file: &JournalFile,
    snapshot_sha: Option<&str>,
    label: &str,
) -> Result<(), String> {
    if !file.existed {
        if file.target.exists() {
            return Err(format!(
                "config_changed_concurrently: {label}在事务期间被外部创建"
            ));
        }
        return Ok(());
    }

    let current_bytes =
        fs::read(&file.target).map_err(|e| format!("重新读取{label}以复核快照失败: {e}"))?;
    if Some(sha256_hex(&current_bytes).as_str()) != snapshot_sha {
        return Err(format!(
            "config_changed_concurrently: {label}在事务期间被外部修改"
        ));
    }
    Ok(())
}

fn remove_journal_temp_files(journal: &SetupJournal) {
    if let Some(ref temp) = journal.config.temp {
        let _ = fs::remove_file(temp);
    }
    if let Some(ref temp) = journal.env.temp {
        let _ = fs::remove_file(temp);
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let result = hasher.finalize();
    let mut s = String::with_capacity(64);
    for b in result {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepare_rejects_a_target_created_after_the_transaction_begins() {
        let root = axon_test_support::test_temp_root().join(format!(
            "axon-setup-transaction-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let config_path = root.join("config.toml");
        let env_path = root.join(".env");
        let mut transaction = SetupTransaction::begin(&config_path, &env_path).unwrap();
        fs::write(&config_path, "[remote]\nenabled = true\n").unwrap();

        let error = transaction.prepare(Some(b"[remote]\nenabled = false\n"), None);

        assert!(error.unwrap_err().contains("config_changed_concurrently"));
        assert_eq!(
            fs::read_to_string(&config_path).unwrap(),
            "[remote]\nenabled = true\n"
        );
        transaction.abort_prepare();
        let _ = fs::remove_dir_all(root);
    }
}
